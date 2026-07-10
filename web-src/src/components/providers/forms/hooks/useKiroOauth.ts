import { useManagedAuth } from "./useManagedAuth";

export function useKiroOauth() {
  const auth = useManagedAuth("kiro_oauth");
  return {
    ...auth,
    addSocialAccount: (provider: "google" | "github") =>
      auth.addAccountWithMode(undefined, { kiroLoginProvider: provider }),
  };
}
