import { useManagedAuth } from "./useManagedAuth";
import type { ManagedAuthProvider } from "@/lib/api";

export function useAntigravityOauth(
  authProvider: Extract<ManagedAuthProvider, "antigravity_oauth" | "agy_oauth"> =
    "antigravity_oauth",
) {
  return useManagedAuth(authProvider);
}
