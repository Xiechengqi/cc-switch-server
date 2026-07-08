// AUTO-GENERATED at build time by scripts/fetch-regions.mjs. Do not edit manually.
// Source: https://raw.githubusercontent.com/Xiechengqi/cc-switch-router/refs/heads/master/regions

export interface ShareRegion {
  region: string;
  baseUrl: string;
}

export const SHARE_REGIONS: ShareRegion[] = [
  { region: "japan", baseUrl: "jptokenswitch.cc" },
  { region: "singapore", baseUrl: "sgptokenswitch.cc" },
];

export const DEFAULT_SHARE_ROUTER_DOMAIN =
  SHARE_REGIONS[0]?.baseUrl ?? "jptokenswitch.cc";
