#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const upstream = process.env.CC_SWITCH_UPSTREAM_DIR || "/data/projects/cc-switch-offical";
const args = process.argv.slice(2);
const failOnMustReview = args.includes("--fail-on-must-review");
const range = args.find((arg) => !arg.startsWith("--")) || "HEAD~20..HEAD";

const mustReview = [
  /^src-tauri\/src\/proxy\/forwarder\.rs$/,
  /^src-tauri\/src\/proxy\/handlers\.rs$/,
  /^src-tauri\/src\/proxy\/providers\//,
  /^src-tauri\/src\/services\/subscription\.rs$/,
  /^src-tauri\/src\/services\/oauth_quota\.rs$/,
  /^src-tauri\/src\/services\/usage_stats\.rs$/,
  /^src-tauri\/src\/provider\.rs$/,
  /^src-tauri\/src\/services\/provider\//,
  /^src-tauri\/src\/database\/schema\.rs$/,
  /^src\/config\/(claude|codex|gemini)ProviderPresets\.ts$/,
  /^src\/config\/universalProviderPresets\.ts$/,
];

const optionalReview = [
  /^src-tauri\/src\/web\//,
  /^src-tauri\/src\/tunnel\//,
  /^src-tauri\/src\/services\/share\.rs$/,
  /^src-tauri\/src\/commands\/share\.rs$/,
  /^src\/components\/providers\//,
  /^src\/components\/share\//,
  /^src\/components\/usage\//,
];

function git(args) {
  return execFileSync("git", args, {
    cwd: upstream,
    encoding: "utf8",
  }).trim();
}

function classify(file) {
  if (mustReview.some((pattern) => pattern.test(file))) return "must-review";
  if (optionalReview.some((pattern) => pattern.test(file))) return "optional";
  return "ignore";
}

function ledgerCommitHashes() {
  const ledgerPath = path.resolve("UPSTREAM_IMPORT.md");
  if (!fs.existsSync(ledgerPath)) return new Set();
  const content = fs.readFileSync(ledgerPath, "utf8");
  return new Set([...content.matchAll(/`([0-9a-f]{8,40})`/g)].map((match) => match[1]));
}

function commitLogged(hash, logged) {
  return [...logged].some((entry) => entry.startsWith(hash) || hash.startsWith(entry));
}

function mustReviewCommits(range, files) {
  if (files.length === 0) return [];
  return git(["log", "--format=%h %s", range, "--", ...files])
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [hash, ...messageParts] = line.split(/\s+/);
      return { hash, message: messageParts.join(" ") };
    });
}

function main() {
  const root = path.resolve(upstream);
  const files = git(["diff", "--name-only", range])
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  const commits = git(["log", "--oneline", range])
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  const rows = files.map((file) => ({
    file,
    classification: classify(file),
  }));

  console.log(`# Upstream change scan`);
  console.log(`upstream: ${root}`);
  console.log(`range: ${range}`);
  console.log(`commits: ${commits.length}`);
  if (commits.length > 0) {
    console.log("");
    console.log("| Commit |");
    console.log("| --- |");
    for (const commit of commits) {
      console.log(`| \`${commit}\` |`);
    }
  }
  console.log("");
  console.log("| Classification | Path |");
  console.log("| --- | --- |");
  for (const row of rows) {
    console.log(`| ${row.classification} | \`${row.file}\` |`);
  }

  const mustReviewFiles = rows
    .filter((row) => row.classification === "must-review")
    .map((row) => row.file);
  const reviewCommits = mustReviewCommits(range, mustReviewFiles);
  const logged = ledgerCommitHashes();
  const unloggedReviewCommits = reviewCommits.filter((commit) => !commitLogged(commit.hash, logged));
  if (mustReviewFiles.length > 0) {
    console.log("");
    console.log(`must-review count: ${mustReviewFiles.length}`);
  }
  if (reviewCommits.length > 0) {
    console.log("");
    console.log("| Must Review Commit | Ledger |");
    console.log("| --- | --- |");
    for (const commit of reviewCommits) {
      const status = commitLogged(commit.hash, logged) ? "recorded" : "missing";
      console.log(`| \`${commit.hash} ${commit.message}\` | ${status} |`);
    }
  }
  if (failOnMustReview && unloggedReviewCommits.length > 0) {
    process.exitCode = 2;
  }
}

main();
