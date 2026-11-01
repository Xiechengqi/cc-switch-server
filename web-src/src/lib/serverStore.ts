import type { ServerStoreAuthority } from "./server-legacy-api";

/**
 * What the storage settings card shows and offers:
 * - `committed`: SQLite is the only store; nothing to do.
 * - `migrating`: SQLite is prepared and commits on the next start.
 * - `pending`: an owner asked to migrate; it runs on the next start.
 * - `failed`: the last attempt failed; the Server kept the JSON files.
 * - `available`: not migrated and nothing requested yet.
 */
export type ServerStorePhase =
  | "committed"
  | "migrating"
  | "pending"
  | "failed"
  | "available";

export function serverStorePhase(
  store: Pick<ServerStoreAuthority, "authority" | "activationRequest">,
): ServerStorePhase {
  if (store.authority === "committed") return "committed";
  // A failure needs the owner's attention even if a later attempt is prepared.
  if (store.activationRequest?.status === "failed") return "failed";
  if (store.authority === "prepared") return "migrating";
  if (store.activationRequest?.status === "pending") return "pending";
  return "available";
}

const AUTHORITY_STATES = ["legacy", "shadow_verified", "prepared", "committed"];

/** Plain-words label for the authority; an unknown value gets a generic label. */
export function serverStoreAuthorityKey(authority: string | null | undefined): string {
  return `settings.serverStorage.authority.${
    authority && AUTHORITY_STATES.includes(authority) ? authority : "unknown"
  }`;
}

/** The SQLite shadow is behind the JSON files; the migration rebuilds it. */
export function serverStoreShadowStale(
  store: Pick<ServerStoreAuthority, "authority" | "shadowCurrent">,
): boolean {
  return store.authority !== "committed" && store.shadowCurrent === false;
}
