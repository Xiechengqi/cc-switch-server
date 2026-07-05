import type { ShareFilter, ShareSort } from "@/components/share/ShareToolbar";
import type { ShareRecord } from "@/lib/api";

export function filterShares(
  shares: ShareRecord[],
  query: string,
  filter: ShareFilter,
  sort: ShareSort,
): ShareRecord[] {
  const normalizedQuery = query.trim().toLowerCase();
  return shares.filter((share) => {
    if (filter === "active" && share.status !== "active") return false;
    if (filter === "paused" && share.status !== "paused") return false;
    if (filter === "expired" && share.status !== "expired") return false;
    if (filter === "exhausted" && share.status !== "exhausted") return false;
    if (filter === "sale" && !share.forSale) return false;
    if (!normalizedQuery) return true;
    return [
      share.id,
      share.displayName,
      share.ownerEmail,
      share.status,
      share.tunnelSubdomain,
      share.description,
      share.saleMarketKind,
      share.app,
      share.providerId,
      share.providerType,
      ...(share.bindings || []).map((binding) => `${binding.app} ${binding.providerId} ${binding.providerType}`),
    ]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(normalizedQuery);
  }).sort((left, right) => compareShares(left, right, sort));
}

function compareShares(left: ShareRecord, right: ShareRecord, sort: ShareSort): number {
  if (sort === "expiresAtAsc") {
    return compareNullableNumber(left.expiresAt, right.expiresAt, "asc");
  }
  if (sort === "tokensUsedDesc") {
    return compareNumber(right.tokensUsed, left.tokensUsed) || compareShareName(left, right);
  }
  if (sort === "nameAsc") {
    return compareShareName(left, right);
  }
  return compareNullableNumber(shareCreatedAtMs(left), shareCreatedAtMs(right), "desc");
}

function shareCreatedAtMs(share: ShareRecord): number | null {
  return normalizeTimeMs(
    share.createdAtMs ??
      share.createdAt ??
      share.created_at_ms ??
      share.created_at,
  );
}

function normalizeTimeMs(value: number | string | null | undefined): number | null {
  if (typeof value === "number") {
    if (!Number.isFinite(value)) return null;
    return value > 0 && value < 10_000_000_000 ? value * 1000 : value;
  }
  if (typeof value !== "string" || !value.trim()) return null;
  const numeric = Number(value);
  if (Number.isFinite(numeric)) {
    return numeric > 0 && numeric < 10_000_000_000 ? numeric * 1000 : numeric;
  }
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? parsed : null;
}

function compareNullableNumber(
  left: number | null | undefined,
  right: number | null | undefined,
  direction: "asc" | "desc",
): number {
  const leftValid = typeof left === "number" && Number.isFinite(left);
  const rightValid = typeof right === "number" && Number.isFinite(right);
  if (!leftValid && !rightValid) return 0;
  if (!leftValid) return 1;
  if (!rightValid) return -1;
  return direction === "asc" ? compareNumber(left, right) : compareNumber(right, left);
}

function compareNumber(left: number | null | undefined, right: number | null | undefined): number {
  return (left || 0) - (right || 0);
}

function compareShareName(left: ShareRecord, right: ShareRecord): number {
  const leftName = (left.displayName || left.id || "").toLowerCase();
  const rightName = (right.displayName || right.id || "").toLowerCase();
  return leftName.localeCompare(rightName);
}
