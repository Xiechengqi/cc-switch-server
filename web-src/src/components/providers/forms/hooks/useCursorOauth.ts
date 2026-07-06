import { useManagedAuth } from "./useManagedAuth";

export function useCursorOauth() {
  return useManagedAuth("cursor_oauth");
}
