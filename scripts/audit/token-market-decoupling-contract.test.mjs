import assert from "node:assert/strict";
import test from "node:test";
import {
  retiredMarketEndpointPattern,
  retainedRouterMarketFiles,
  routerShareReaderErrors,
} from "./token-market-decoupling-contract.mjs";

test("wallet FX config is allowed without exempting retired Market routes", () => {
  for (const route of ["/v1/market/config", "/v1/market/config?locale=en", "/v1/market-fx"]) {
    assert.equal(retiredMarketEndpointPattern.test(JSON.stringify(route)), false, route);
  }
  for (const route of [
    "/v1/markets", "/v1/markets/register", "/v1/market/register",
    "/v1/market/config/legacy", "/v1/market/configuration", "/_market/proxy",
  ]) {
    assert.equal(retiredMarketEndpointPattern.test(JSON.stringify(route)), true, route);
  }
  assert.equal(retiredMarketEndpointPattern.test(
    'fetch("/v1/market/config"); fetch("/v1/market/register")',
  ), true, "a valid FX route must not exempt another retired route in the same file");
});

const models = `
pub const MIN_SHARE_CONTRACT_VERSION: u16 = 2;
pub const MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 5;
pub const SHARE_CONTRACT_VERSION: u16 = 10;
`;
test("reader ceiling upgrades retain the separate reader and Market floors", () => {
  for (const version of [8, 9, 10, 11]) {
    assert.deepEqual(routerShareReaderErrors(models.replace("= 10;", `= ${version};`)), []);
  }
  for (const [from, to] of [
    ["MIN_SHARE_CONTRACT_VERSION: u16 = 2", "MIN_SHARE_CONTRACT_VERSION: u16 = 3"],
    ["MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 5", "MIN_SHARE_MARKET_CONTRACT_VERSION: u16 = 10"],
    ["SHARE_CONTRACT_VERSION: u16 = 10", "SHARE_CONTRACT_VERSION: u16 = 7"],
    ["SHARE_CONTRACT_VERSION: u16 = 10", "SHARE_CONTRACT_VERSION: u16 = 65536"],
    ["pub const SHARE_CONTRACT_VERSION", "// pub const SHARE_CONTRACT_VERSION"],
  ]) {
    assert.notDeepEqual(routerShareReaderErrors(models.replace(from, to)), []);
  }
});

test("retained Market capability checks follow the platform-wallet architecture", () => {
  assert.ok(retainedRouterMarketFiles.includes("src/market_wallet.rs"));
  assert.ok(retainedRouterMarketFiles.includes("src/market_payments/mod.rs"));
  assert.ok(!retainedRouterMarketFiles.includes("src/market_billing.rs"));
});
