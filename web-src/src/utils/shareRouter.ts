import { SHARE_REGIONS } from "@/config/shareRegions";

export const CUSTOM_SHARE_ROUTER_VALUE = "__custom_share_router__";

export function findShareRouterRegion(domain: string) {
  const normalized = normalizeShareRouterDomainForCompare(domain);
  return SHARE_REGIONS.find((region) => region.baseUrl === normalized);
}

export function formatShareRouterDisplay(domain: string): string {
  const normalized = normalizeShareRouterDomainForCompare(domain);
  const region = SHARE_REGIONS.find((item) => item.baseUrl === normalized);
  return region ? `${region.region} - ${region.baseUrl}` : domain;
}

export function normalizeShareRouterDomainForCompare(domain: string): string {
  return domain.trim().replace(/\/+$/, "").toLowerCase();
}

export function normalizeShareRouterDomain(input: string): string {
  const trimmed = input.trim().replace(/\/+$/, "");
  if (!trimmed) {
    throw new Error("share.validation.required");
  }
  if (/\s/.test(trimmed)) {
    throw new Error("share.validation.invalidRouterDomain");
  }

  let authority: string;
  const lower = trimmed.toLowerCase();
  if (lower.startsWith("http://") || lower.startsWith("https://")) {
    let parsed: URL;
    try {
      parsed = new URL(trimmed);
    } catch {
      throw new Error("share.validation.invalidRouterDomain");
    }
    if (
      parsed.username ||
      parsed.password ||
      parsed.pathname !== "/" ||
      parsed.search ||
      parsed.hash
    ) {
      throw new Error("share.validation.invalidRouterDomain");
    }
    authority = parsed.host;
  } else {
    if (
      trimmed.includes("://") ||
      trimmed.includes("/") ||
      trimmed.includes("?") ||
      trimmed.includes("#")
    ) {
      throw new Error("share.validation.invalidRouterDomain");
    }
    authority = trimmed;
  }

  authority = authority.toLowerCase();
  validateShareRouterAuthority(authority);
  return authority;
}

function validateShareRouterAuthority(authority: string) {
  if (
    !authority ||
    authority.length > 253 ||
    authority.includes("@") ||
    authority.includes("[") ||
    authority.includes("]")
  ) {
    throw new Error("share.validation.invalidRouterDomain");
  }

  const portMatch = authority.match(/^(.*):(\d+)$/);
  const host = portMatch ? portMatch[1] : authority;
  const port = portMatch ? Number(portMatch[2]) : null;
  if (port !== null && (!Number.isInteger(port) || port < 1 || port > 65535)) {
    throw new Error("share.validation.invalidRouterDomain");
  }

  if (["localhost", "127.0.0.1", "0.0.0.0"].includes(host)) {
    return;
  }
  if (host === "example.com" || host.endsWith(".example.com")) {
    throw new Error("share.validation.invalidRouterDomain");
  }
  if (/^(?:\d{1,3}\.){3}\d{1,3}$/.test(host)) {
    if (
      host.split(".").every((part) => Number(part) >= 0 && Number(part) <= 255)
    ) {
      return;
    }
    throw new Error("share.validation.invalidRouterDomain");
  }

  if (!host.includes(".")) {
    throw new Error("share.validation.invalidRouterDomain");
  }
  for (const label of host.split(".")) {
    if (
      !label ||
      label.length > 63 ||
      label.startsWith("-") ||
      label.endsWith("-") ||
      !/^[a-z0-9-]+$/.test(label)
    ) {
      throw new Error("share.validation.invalidRouterDomain");
    }
  }
}
