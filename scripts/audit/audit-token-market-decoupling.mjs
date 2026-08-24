#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const serverRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const routerRoot = path.resolve(
  process.env.CC_SWITCH_ROUTER_AUDIT_ROOT ||
    path.join(serverRoot, "../cc-switch-router"),
);

const failures = [];
const retiredShareFieldPattern =
  /forSale|for_sale|officialPrice|official_price|sharedWithEmails|shared_with_emails|marketAccessMode|market_access_mode|accessByApp|access_by_app|appSettings|app_settings/;
// Match a retired Share key as a serialized/JSON key.  The identifier-only
// rule above intentionally stays narrow because names such as `marketEmail`
// still occur as legitimate Client Market owner metadata.  This key-focused
// rule closes the audit gap without treating those unrelated local variables
// as retired Share wire fields.
const retiredShareKeyPattern =
  /["'`](?:acl|forSale|for_sale|officialPricePercent|official_price_percent|forSaleOfficialPricePercentByApp|for_sale_official_price_percent_by_app|sharedWithEmails|shared_with_emails|marketAccessMode|market_access_mode|accessByApp|access_by_app|appSettings|app_settings|publicMarketEmail|public_market_email|marketEmail|market_email|marketSubdomain|market_subdomain|marketUrl|market_url|marketId|market_id|saleMarketKind|sale_market_kind)["'`]/;
const retiredShareKeys = new Set([
  "acl",
  "forSale",
  "for_sale",
  "officialPricePercent",
  "official_price_percent",
  "forSaleOfficialPricePercentByApp",
  "for_sale_official_price_percent_by_app",
  "sharedWithEmails",
  "shared_with_emails",
  "marketAccessMode",
  "market_access_mode",
  "accessByApp",
  "access_by_app",
  "appSettings",
  "app_settings",
  "publicMarketEmail",
  "public_market_email",
  "marketEmail",
  "market_email",
  "marketSubdomain",
  "market_subdomain",
  "marketUrl",
  "market_url",
  "marketId",
  "market_id",
  "saleMarketKind",
  "sale_market_kind",
]);

function fail(message) {
  failures.push(message);
}

function relative(root, file) {
  return path.relative(root, file).split(path.sep).join("/");
}

function requireDirectory(directory, label) {
  if (!fs.existsSync(directory) || !fs.statSync(directory).isDirectory()) {
    fail(`${label} is unavailable: ${directory}`);
    return false;
  }
  return true;
}

function sourceFiles(root, entries) {
  const files = [];
  const visit = (entry) => {
    if (!fs.existsSync(entry)) return;
    const stat = fs.statSync(entry);
    if (stat.isFile()) {
      files.push(entry);
      return;
    }
    for (const child of fs.readdirSync(entry, { withFileTypes: true })) {
      if (
        child.name === "node_modules" ||
        child.name === "target" ||
        child.name === ".next" ||
        child.name === "out" ||
        child.name === "web-dist"
      ) {
        continue;
      }
      visit(path.join(entry, child.name));
    }
  };
  for (const entry of entries) visit(path.join(root, entry));
  return files.sort();
}

function auditRules(root, files, rules) {
  for (const file of files) {
    const fileRelative = relative(root, file);
    const content = fs.readFileSync(file, "utf8");
    for (const rule of rules) {
      rule.pattern.lastIndex = 0;
      if (!rule.pattern.test(content)) continue;
      if ((rule.allowedFiles || []).includes(fileRelative)) continue;
      fail(`${fileRelative}: retired Token Market residue matched ${rule.label}`);
    }
  }
}

function requireText(root, fileRelative, pattern, label) {
  const file = path.join(root, fileRelative);
  if (!fs.existsSync(file)) {
    fail(`${label} file is missing: ${fileRelative}`);
    return;
  }
  const content = fs.readFileSync(file, "utf8");
  pattern.lastIndex = 0;
  if (!pattern.test(content)) fail(`${fileRelative}: missing ${label}`);
}

function requireAllText(root, fileRelative, expectations) {
  for (const [pattern, label] of expectations) {
    requireText(root, fileRelative, pattern, label);
  }
}

function requireBlock(root, fileRelative, startMarker, endMarker, contract) {
  const file = path.join(root, fileRelative);
  if (!fs.existsSync(file)) {
    fail(`${contract.label} file is missing: ${fileRelative}`);
    return;
  }
  const content = fs.readFileSync(file, "utf8");
  const start = content.indexOf(startMarker);
  const end =
    start < 0
      ? -1
      : content.indexOf(endMarker, start + startMarker.length);
  if (start < 0 || end < 0) {
    fail(`${fileRelative}: could not locate ${contract.label}`);
    return;
  }
  const block = content.slice(start, end + endMarker.length);
  for (const [pattern, label] of contract.required || []) {
    pattern.lastIndex = 0;
    if (!pattern.test(block)) {
      fail(`${fileRelative}: ${contract.label} is missing ${label}`);
    }
  }
  for (const [pattern, label] of contract.forbidden || []) {
    pattern.lastIndex = 0;
    if (pattern.test(block)) {
      fail(`${fileRelative}: ${contract.label} contains forbidden ${label}`);
    }
  }
}

function requireMissing(root, fileRelative, label) {
  if (fs.existsSync(path.join(root, fileRelative))) {
    fail(`${fileRelative}: retired ${label} must not exist`);
  }
}

function loadJson(root, fileRelative, label) {
  const file = path.join(root, fileRelative);
  if (!fs.existsSync(file)) {
    fail(`${label} file is missing: ${fileRelative}`);
    return undefined;
  }
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    fail(`${fileRelative}: invalid ${label}: ${error.message}`);
    return undefined;
  }
}

function auditShareFixture(root, fileRelative, shareSelector) {
  const fixture = loadJson(root, fileRelative, "Share Contract fixture");
  if (!fixture) return;
  const share = shareSelector(fixture);
  if (!share || typeof share !== "object" || Array.isArray(share)) {
    fail(`${fileRelative}: Share Contract fixture has no Share object`);
    return;
  }
  if (share.contractVersion !== 2) {
    fail(`${fileRelative}: Share Contract fixture must use contractVersion=2`);
  }
  for (const key of Object.keys(share)) {
    if (retiredShareKeys.has(key)) {
      fail(`${fileRelative}: Share Contract v2 fixture contains retired field ${key}`);
    }
  }
}

if (requireDirectory(serverRoot, "Server audit root")) {
  const serverFiles = sourceFiles(serverRoot, [
    "src",
    "web-src/src",
    "scripts/smoke",
    "assets/contract/web-runtime-contract.json",
    ".env.example",
  ]);
  auditRules(serverRoot, serverFiles, [
    {
      label: "Token Market discovery/API symbol",
      pattern:
        /list_token_markets|PublicTokenMarket|ListTokenMarketsResponse|\/api\/token-markets/,
    },
    {
      label: "retired Market public-host namespace",
      pattern: /MarketSlug|PublicHostKind::Market|PublicHost::for_market/,
    },
    {
      label: "retired Market endpoint or tunnel type",
      pattern: /\/v1\/markets|\/v1\/market\/|\/_market\/proxy|market-http/,
      allowedFiles: ["scripts/smoke/router-share-smoke.sh"],
    },
    {
      label: "retired standalone Market environment",
      pattern: /\bMARKET_URL\b|\bMARKET_API_URL\b/,
    },
    {
      label: "legacy public Market identity outside the one-time migration",
      pattern: /publicMarketEmail|public_market_email/,
      allowedFiles: [
        "src/domain/sharing/legacy_token_market_migration.rs",
        "src/domain/sharing/retired_fields.rs",
        "src/api/types/shares.rs",
      ],
    },
    {
      label: "retired Share wire/UI field",
      pattern: retiredShareFieldPattern,
      allowedFiles: [
        "src/domain/sharing/legacy_token_market_migration.rs",
        "src/domain/sharing/retired_fields.rs",
        "src/api/types/shares.rs",
        "src/api/invoke/handlers.rs",
      ],
    },
    {
      label: "retired Share key literal",
      pattern: retiredShareKeyPattern,
      allowedFiles: [
        "src/domain/sharing/legacy_token_market_migration.rs",
        "src/domain/sharing/retired_fields.rs",
        "src/domain/sharing/invariants.rs",
        "src/api/types/shares.rs",
        "src/api/invoke/handlers.rs",
      ],
    },
  ]);

  requireAllText(serverRoot, "src/domain/sharing/legacy_token_market_migration.rs", [
    [/migrate_legacy_share_contract/, "one-time legacy Share contract migration"],
    [/remove_retired_archive_payload/, "physical removal of retired archive payloads"],
    [/source_sha256/, "non-identifying source checksum receipt"],
    [/data-retirement-audit\.json/, "data retirement audit receipt"],
  ]);
  requireAllText(serverRoot, "src/domain/sharing/retired_fields.rs", [
    [/RETIRED_SHARE_FIELDS/, "central retired Share field denylist"],
    [/find_retired_share_field/, "recursive retired-field detection"],
  ]);
  requireAllText(serverRoot, "src/clients/router/control_store.rs", [
    [/const SCHEMA_VERSION: i64 = 3;/, "Router control schema v3"],
    [/kind IN \('client', 'share'\)/, "Client/Share-only local public-host schema"],
    [/fn migrate_schema_v2_to_v3[\s\S]*DROP TABLE legacy_token_market_public_hosts;[\s\S]*DROP TABLE legacy_token_market_archive_manifest;/, "verified physical removal of legacy local host archives"],
  ]);
  requireAllText(serverRoot, "src/api/types/shares.rs", [
    [/find_retired_share_field/, "REST retired-field denylist"],
    [/retired Share field/, "REST fail-closed retired-field error"],
  ]);
  requireText(
    serverRoot,
    "src/domain/sharing/router_contract.rs",
    /pub const SHARE_CONTRACT_VERSION: u16 = 3;/,
    "Share Contract v3 constant",
  );
  requireBlock(
    serverRoot,
    "src/domain/sharing/router_contract.rs",
    "pub struct ShareDescriptor {",
    "\n}",
    {
      label: "Server ShareDescriptor v3",
      required: [
        [/contract_version/, "contract version"],
        [/free_access/, "canonical free access"],
        [/user_grants/, "canonical user grants"],
      ],
      forbidden: [
        [retiredShareFieldPattern, "legacy Share sale/ACL fields"],
        [retiredShareKeyPattern, "legacy Share key literals"],
      ],
    },
  );
  requireBlock(
    serverRoot,
    "src/domain/sharing/router_contract.rs",
    "pub struct ShareSettingsPatch {",
    "\n}",
    {
      label: "Server ShareSettingsPatch v3",
      required: [
        [/free_access/, "canonical free access"],
        [/user_grants/, "canonical user grants"],
      ],
      forbidden: [
        [retiredShareFieldPattern, "legacy Share sale/ACL fields"],
        [retiredShareKeyPattern, "legacy Share key literals"],
      ],
    },
  );
  requireBlock(
    serverRoot,
    "src/domain/sharing/router_contract.rs",
    "pub struct ShareUserPolicy {",
    "\n}",
    {
      label: "Server ShareUserPolicy v3",
      required: [[/allowed_apps/, "market App scope"]],
    },
  );
  requireBlock(
    serverRoot,
    "src/domain/sharing/shares.rs",
    "pub struct Share {",
    "\n}",
    {
      label: "Server persisted Share model",
      required: [[/user_grants/, "canonical user grants"]],
      forbidden: [
        [retiredShareFieldPattern, "legacy Share sale/ACL fields"],
        [retiredShareKeyPattern, "legacy Share key literals"],
      ],
    },
  );

  requireMissing(
    serverRoot,
    "scripts/smoke/router-market-smoke.sh",
    "standalone Market smoke",
  );
  requireMissing(
    serverRoot,
    "scripts/smoke/direct-market-diagnostics.sh",
    "direct Market diagnostics",
  );
}

if (requireDirectory(routerRoot, "Router audit root")) {
  const routerFiles = sourceFiles(routerRoot, [
    "src",
    "frontend/app",
    "frontend/components",
    "frontend/lib",
  ]);
  auditRules(routerRoot, routerFiles, [
    {
      label: "retired Market registry/auth/proxy type",
      pattern:
        /MarketRegistryRecord|PublicMarketConfig|RegisterMarketRequest|authenticate_market|market_proxy_handler|MarketSlug|PublicHostKind::Market/,
    },
    {
      label: "retired Market database table outside migration verification",
      pattern:
        /\brouter_markets\b|\bmarket_request_logs\b|\bmarket_disabled_shares\b|\bmarket_share_model_failure_state\b|\bmarket_share_runtime_states\b|\bmarket_notification_emails\b/,
      allowedFiles: ["src/schema.rs"],
    },
    {
      label: "retired Market endpoint outside the explicit 410 router",
      pattern: /\/v1\/markets|\/v1\/market\/|\/_market\/proxy/,
      allowedFiles: ["src/api.rs"],
    },
    {
      label: "non-wire Gateway body hashing or self-reported email authorization",
      pattern: /json_body_sha256_hex|principal:\s*&gateway\.owner_email/,
    },
    {
      label: "retired Share wire/UI field outside frozen storage compatibility",
      pattern: retiredShareFieldPattern,
      allowedFiles: ["src/schema.rs", "src/store.rs", "src/share_market.rs"],
    },
    {
      label: "retired Share key literal outside compatibility boundaries",
      pattern: retiredShareKeyPattern,
      allowedFiles: [
        "src/schema.rs",
        "src/store.rs",
        "src/share_market.rs",
        "src/client_market.rs",
        "src/metrics/store.rs",
      ],
    },
  ]);

  for (const retiredComponent of [
    "frontend/components/dashboard/markets-page.tsx",
    "frontend/components/dashboard/markets-table.tsx",
  ]) {
    requireMissing(routerRoot, retiredComponent, "Token Market UI component");
  }
  requireText(
    routerRoot,
    "src/api.rs",
    /retired_token_market_routes[\s\S]*StatusCode::GONE/,
    "explicit 410 retirement router",
  );
  requireText(
    routerRoot,
    "src/models.rs",
    /pub const SHARE_CONTRACT_VERSION: u16 = 3;/,
    "Share Contract v3 constant",
  );
  requireBlock(
    routerRoot,
    "src/models.rs",
    "pub struct GatewayShareView {",
    "\n}",
    {
      label: "Gateway Share privacy boundary",
      required: [
        [
          /#\[serde\(skip\)\][\s\S]*pub\(crate\) scheduling_owner_email/,
          "non-serialized internal scheduling owner",
        ],
      ],
      forbidden: [
        [/pub\s+owner_email\s*:/, "serialized Share owner email"],
        [/installation_owner_email/, "installation owner email"],
      ],
    },
  );
  requireAllText(routerRoot, "src/store.rs", [
    [/share_name: gateway_share_label\(/, "opaque Gateway share label"],
    [
      /fn redact_gateway_provider[\s\S]*account_email = None;[\s\S]*api_url = None;/,
      "Gateway Provider identity redaction",
    ],
    [/fn redact_gateway_app_runtimes/, "Gateway runtime identity redaction"],
    [/fn gateway_share_label[\s\S]*Sha256::digest/, "stable opaque share label derivation"],
  ]);
  requireText(
    routerRoot,
    "src/models.rs",
    /serde\(rename_all = "camelCase", deny_unknown_fields\)\]\npub struct ShareDescriptor/,
    "strict ShareDescriptor unknown-field rejection",
  );
  requireBlock(
    routerRoot,
    "src/models.rs",
    "pub struct ShareDescriptor {",
    "\n}",
    {
      label: "Router ShareDescriptor v3",
      required: [
        [/contract_version/, "contract version"],
        [/free_access/, "canonical free access"],
        [/user_grants/, "canonical user grants"],
      ],
      forbidden: [
        [retiredShareFieldPattern, "legacy Share sale/ACL fields"],
        [retiredShareKeyPattern, "legacy Share key literals"],
      ],
    },
  );
  requireBlock(
    routerRoot,
    "src/models.rs",
    "pub struct ShareSettingsPatch {",
    "\n}",
    {
      label: "Router ShareSettingsPatch v3",
      required: [
        [/free_access/, "canonical free access"],
        [/user_grants/, "canonical user grants"],
      ],
      forbidden: [
        [retiredShareFieldPattern, "legacy Share sale/ACL fields"],
        [retiredShareKeyPattern, "legacy Share key literals"],
      ],
    },
  );
  requireBlock(
    routerRoot,
    "src/models.rs",
    "pub struct ShareUserPolicy {",
    "\n}",
    {
      label: "Router ShareUserPolicy v3",
      required: [[/allowed_apps/, "market App scope"]],
    },
  );
  requireAllText(routerRoot, "src/store.rs", [
    [/let shared_with_emails_json = "\[\]";/, "empty legacy ACL storage write"],
    [/let access_by_app_json = "\{\}";/, "empty legacy per-app ACL storage write"],
    [/let app_settings_json = "\{\}";/, "empty legacy app settings storage write"],
    [/params!\[[\s\S]*"selected",[\s\S]*"No",[\s\S]*share\.free_access/, "fixed legacy storage sentinels and canonical free access"],
    [/fn list_shares[\s\S]*COALESCE\(s\.user_grants_json, '\{\}'\)[\s\S]*COALESCE\(s\.free_access, 0\)/, "canonical Share read projection"],
    [/parse canonical Share user grants failed/, "fail-closed canonical grant decoding"],
  ]);
  requireText(
    routerRoot,
    "src/share_market.rs",
    /SET user_grants_json = \?2, shared_with_emails_json = '\[\]'/,
    "Share Market writes only canonical grants and clears legacy ACL storage",
  );

  requireAllText(routerRoot, "schema/0021_physically_retire_legacy_token_market.sql", [
    [/CREATE TABLE IF NOT EXISTS data_retirement_audit/, "non-identifying retirement receipt"],
    [/DROP TABLE legacy_token_market_archive_manifest;/, "archive manifest drop"],
    [/DROP TABLE legacy_token_market_router_markets;/, "archived registry drop"],
    [/DROP TABLE legacy_token_market_public_hosts;/, "archived host drop"],
    [/DROP TABLE legacy_token_market_notification_emails;/, "archived notification drop"],
    [/DROP TABLE legacy_token_market_request_logs;/, "archived request log drop"],
    [/DROP TABLE legacy_token_market_disabled_shares;/, "archived disabled Share drop"],
    [/DROP TABLE legacy_token_market_share_model_failure_state;/, "archived model failure drop"],
    [/DROP TABLE legacy_token_market_share_runtime_states;/, "archived runtime state drop"],
    [/DROP TABLE router_markets;/, "live registry drop"],
    [/DROP TABLE market_notification_emails;/, "live notification drop"],
    [/DROP TABLE market_request_logs;/, "live request log drop"],
    [/DROP TABLE market_disabled_shares;/, "live disabled Share drop"],
    [/DROP TABLE market_share_model_failure_state;/, "live model failure drop"],
    [/DROP TABLE market_share_runtime_states;/, "live runtime state drop"],
  ]);
  requireText(
    routerRoot,
    "schema/0019_retire_legacy_token_market.sql",
    /CREATE VIEW IF NOT EXISTS capacity_request_observations[\s\S]*NULL AS user_email[\s\S]*'gateway' AS source_kind/,
    "migration-19 Gateway observations keep terminal user identity null",
  );
  requireText(
    routerRoot,
    "src/schema.rs",
    /version == LEGACY_TOKEN_MARKET_PHYSICAL_RETIREMENT_VERSION[\s\S]*validate_legacy_token_market_archive/,
    "archive verification immediately before physical retirement",
  );
  requireBlock(
    routerRoot,
    "schema/0021_physically_retire_legacy_token_market.sql",
    "CREATE VIEW capacity_request_observations AS",
    "  FROM gateway_request_observations;",
    {
      label: "Gateway-only capacity observation view",
      required: [
        [/'gateway' AS source_kind/, "Gateway source kind"],
        [/NULL AS user_email/, "null terminal-user identity"],
      ],
      forbidden: [
        [/legacy_token_market|market_email/, "legacy Market source or identity"],
        [/gateway_id AS user_email/, "Gateway ID projected as terminal-user email"],
      ],
    },
  );
  requireBlock(
    routerRoot,
    "src/metrics/store.rs",
    "CREATE TABLE IF NOT EXISTS llm_request_metrics (",
    "\n        );",
    {
      label: "canonical metrics table",
      required: [[/gateway_id TEXT/, "Gateway identity"]],
      forbidden: [[/market_email/, "legacy Market identity"]],
    },
  );
  requireText(
    routerRoot,
    "src/metrics/store.rs",
    /fn retire_legacy_llm_request_metrics[\s\S]*DROP TABLE llm_request_metrics;[\s\S]*RENAME TO llm_request_metrics/,
    "physical legacy metrics-column retirement",
  );

  auditShareFixture(
    routerRoot,
    "tests/fixtures/us04_share_lease_request.json",
    (fixture) => fixture.share,
  );
  auditShareFixture(
    routerRoot,
    "tests/fixtures/us04_share_lease_signed_payload.json",
    (fixture) => fixture.share,
  );

  for (const retained of [
    "src/share_market.rs",
    "src/client_market.rs",
    "src/market_access.rs",
    "src/market_billing.rs",
  ]) {
    if (!fs.existsSync(path.join(routerRoot, retained))) {
      fail(`${retained}: retained Router market capability is missing`);
    }
  }
  requireText(
    routerRoot,
    "frontend/app/(dashboard)/markets/page.tsx",
    /redirect\("\/share-market\/"\)/,
    "safe legacy bookmark redirect",
  );
}

if (failures.length) {
  console.error("Token Market decoupling audit failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(
  `Token Market decoupling audit passed (server=${serverRoot}, router=${routerRoot})`,
);
