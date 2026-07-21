import assert from "node:assert/strict";
import crypto from "node:crypto";
import test from "node:test";

import {
  rejectConflictMarkers,
  validateManifestShape,
  validatePinnedManifest,
} from "./sync-desktop-ui.mjs";

const commit = "a".repeat(40);
const serverCommit = "b".repeat(40);
const source = Buffer.from("export const value = true;\n");
const sourceHash = crypto.createHash("sha256").update(source).digest("hex");

function manifest() {
  return {
    schemaVersion: 1,
    upstream: { repository: "/tmp/upstream", commit },
    syncRoots: ["components/ui"],
    serverOwned: [
      {
        path: "components/ui/local.tsx",
        owner: "server",
        reason: "fixture",
        upstreamSourceSha256: sourceHash,
        lastReviewedUpstreamCommit: commit,
        lastReviewedServerCommit: serverCommit,
        exitPhase: "phase-7",
      },
    ],
    excluded: [
      {
        path: "components/ui/desktop-only.tsx",
        kind: "exact",
        reason: "fixture",
      },
    ],
  };
}

test("valid sync manifest verifies its pinned source", () => {
  assert.doesNotThrow(() =>
    validatePinnedManifest(manifest(), {
      resolveCommit: () => commit,
      readSource: () => source,
    }),
  );
});

test("manifest validation rejects malformed and duplicate ownership", () => {
  const malformed = manifest();
  malformed.schemaVersion = 99;
  assert.throws(() => validateManifestShape(malformed), /unsupported.*schema/);

  const duplicate = manifest();
  duplicate.serverOwned.push({ ...duplicate.serverOwned[0] });
  assert.throws(() => validateManifestShape(duplicate), /duplicate server-owned/);

  const conflicting = manifest();
  conflicting.excluded[0].path = conflicting.serverOwned[0].path;
  assert.throws(() => validateManifestShape(conflicting), /both server-owned and excluded/);
});

test("pinned validation rejects stale hashes and missing blobs", () => {
  assert.throws(
    () =>
      validatePinnedManifest(manifest(), {
        resolveCommit: () => commit,
        readSource: () => Buffer.from("changed\n"),
      }),
    /stale upstream hash/,
  );
  assert.throws(
    () =>
      validatePinnedManifest(manifest(), {
        resolveCommit: () => commit,
        readSource: () => null,
      }),
    /stale upstream hash/,
  );
});

test("pinned validation rejects conflict markers before sync", () => {
  assert.throws(
    () => rejectConflictMarkers("fixture.tsx", Buffer.from("<<<<<<< ours\n=======\n>>>>>>> theirs\n")),
    /conflict marker/,
  );
  const conflicted = manifest();
  conflicted.serverOwned[0].upstreamSourceSha256 = crypto
    .createHash("sha256")
    .update("<<<<<<< ours\n=======\n>>>>>>> theirs\n")
    .digest("hex");
  assert.throws(
    () =>
      validatePinnedManifest(conflicted, {
        resolveCommit: () => commit,
        readSource: () => Buffer.from("<<<<<<< ours\n=======\n>>>>>>> theirs\n"),
      }),
    /conflict marker/,
  );
});
