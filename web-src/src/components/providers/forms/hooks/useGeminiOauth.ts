import { useManagedAuth } from "./useManagedAuth";

export function useGeminiOauth() {
  return useManagedAuth("google_gemini_oauth");
}
