// The wallet Market config endpoint is not the retired standalone Token Market.
// Exempt only that exact route, not an entire source file or /v1/market/ prefix.
export const retiredMarketEndpointPattern =
  /\/v1\/markets\b|\/v1\/market\/(?!config(?=["'`?#\s]|$))|\/_market\/proxy/;

export const retainedRouterMarketFiles = [
  "src/share_market.rs",
  "src/client_market.rs",
  "src/market_access.rs",
  "src/market_wallet.rs",
  "src/market_payments/mod.rs",
];

export function routerShareReaderErrors(source) {
  const errors = [];
  for (const [name, floor, exact] of [
    ["MIN_SHARE_CONTRACT_VERSION", 2, true],
    ["MIN_SHARE_MARKET_CONTRACT_VERSION", 5, true],
    ["SHARE_CONTRACT_VERSION", 8, false],
  ]) {
    const match = source.match(
      new RegExp(`^pub const ${name}:\\s*u16\\s*=\\s*(\\d+)\\s*;`, "m"),
    );
    const version = match ? Number(match[1]) : NaN;
    if (
      !Number.isInteger(version) || version > 65535 ||
      (exact ? version !== floor : version < floor)
    ) {
      errors.push(`${name} must ${exact ? "remain at" : "support at least"} v${floor}`);
    }
  }
  return errors;
}
