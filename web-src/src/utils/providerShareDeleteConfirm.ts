const STORAGE_KEY = "cc-switch.provider-share-delete-skip-confirm";

export function isProviderShareDeleteConfirmSkipped(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(STORAGE_KEY) === "1";
}

export function setProviderShareDeleteConfirmSkipped(skip: boolean): void {
  if (typeof window === "undefined") return;
  if (skip) {
    window.localStorage.setItem(STORAGE_KEY, "1");
    return;
  }
  window.localStorage.removeItem(STORAGE_KEY);
}
