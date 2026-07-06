import { useManagedAuth } from "./useManagedAuth";

export function useKiroOauth() {
  return useManagedAuth("kiro_oauth");
}
