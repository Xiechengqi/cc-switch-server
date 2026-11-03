// Explicit release export only. Neither product reads the other's checkout at build/runtime.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import path from "node:path";
import assert from "node:assert/strict";

const root = fileURLToPath(new URL("../", import.meta.url));
const read = (name) => readFileSync(path.join(root, name), "utf8");
const hash = (value) => createHash("sha256").update(value).digest("hex");
const spec = JSON.parse(read("assets/contract/provider-presentation.json"));
const registry = JSON.parse(read("assets/contract/provider-registry.json"));
assert.deepEqual(Object.keys(spec.families).sort(), registry.families.map((f) => f.familyId).sort());
const index = read("web-src/src/icons/extracted/index.ts");
const metadata = read("web-src/src/icons/extracted/metadata.ts");
const icons = {};
for (const match of index.matchAll(/import (\w+) from "\.\/([^"?]+)(\?raw|\?url)?";/g)) {
  const [, binding, filename, mode] = match;
  const id = filename.replace(/\.(svg|png)$/, "");
  const bytes = readFileSync(path.join(root, "web-src/src/icons/extracted", filename));
  const raw = bytes.toString("utf8");
  if (filename.endsWith("svg")) {
    // Only repository-owned, non-active assets can enter the inline renderer.
    assert(!/<(?:script|foreignObject)\b|\bon\w+\s*=|(?:href|src)\s*=\s*["'](?:https?:|\/\/|javascript:)/i.test(raw), filename);
  }
  const block = metadata.match(new RegExp(`(?:"${id}"|\\b${id})\\s*:\\s*\\{([^}]+)\\}`));
  const color = block?.[1].match(/defaultColor:\s*"([^"]+)"/)?.[1];
  icons[id] = {
    mode: mode === "?raw" ? "inline" : "image",
    content: mode === "?raw" ? raw : `data:image/${filename.endsWith("png") ? "png" : "svg+xml"};base64,${bytes.toString("base64")}`,
    ...(color ? { defaultColor: color } : {}),
    sha256: hash(bytes),
  };
  assert(binding);
}
for (const [familyId, family] of Object.entries(spec.families)) {
  assert(Object.hasOwn(spec.categories, family.category), familyId);
  assert(Object.hasOwn(icons, family.icon), familyId);
}
const payload = { ...spec, icons };
const catalogVersion = hash(JSON.stringify(payload));
const catalog = `${JSON.stringify({ ...payload, catalogVersion }, null, 2)}\n`;
const fixtures = registry.families.map((family) => {
  const presentation = spec.families[family.familyId];
  const identity = { schemaVersion: 1, familyId: family.familyId, category: presentation.category,
    displayName: family.label, icon: presentation.icon, iconColor: "#00A67E", catalogVersion, revision: 7 };
  const appProviders = {};
  const bindings = {};
  for (const surface of family.surfaces) {
    bindings[surface.app] = "fixture-provider";
    appProviders[surface.app] = [{ id: "fixture-provider", name: family.label, app: surface.app,
      bundleId: "fixture-private-bundle", providerIdentity: identity }];
  }
  return { name: family.familyId, bindings, appProviders, expectedCategory: presentation.category };
});
const outputs = new Map([
  [path.join(root, "assets/generated/provider-presentation.json"), catalog],
  [path.join(root, "assets/generated/provider-identity-fixtures.json"), `${JSON.stringify(fixtures, null, 2)}\n`],
]);
const exportIndex = process.argv.indexOf("--export-router");
if (exportIndex >= 0) {
  assert(process.argv[exportIndex + 1], "--export-router requires an explicit checkout path");
  outputs.set(path.resolve(process.argv[exportIndex + 1], "assets/provider-presentation.json"), catalog);
  outputs.set(path.resolve(process.argv[exportIndex + 1], "frontend/lib/generated/provider-presentation.json"), catalog);
  outputs.set(path.resolve(process.argv[exportIndex + 1], "assets/provider-identity-fixtures.json"), `${JSON.stringify(fixtures, null, 2)}\n`);
}
for (const [target, content] of outputs) {
  if (process.argv.includes("--check")) assert.equal(readFileSync(target, "utf8"), content, `${target} is stale`);
  else { mkdirSync(path.dirname(target), { recursive: true }); writeFileSync(target, content); }
}
console.log(`Provider presentation: ${Object.keys(spec.families).length} families, ${Object.keys(icons).length} icons, ${hash(catalog).slice(0, 12)}`);
