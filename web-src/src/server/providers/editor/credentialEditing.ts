export type CredentialAction = "keep" | "replace" | "clear";
export type CredentialRevealStatus = "idle" | "loading" | "ready" | "error";

export interface CredentialEdit {
  slot: string;
  configured: boolean;
  action: CredentialAction;
  value: string;
}

export function credentialInputValue(
  edit: CredentialEdit,
  revealedValue?: string,
): string {
  if (edit.action === "keep") return revealedValue ?? "";
  if (edit.action === "replace") return edit.value;
  return "";
}

export function updateCredentialInput(
  edit: CredentialEdit,
  nextValue: string,
  options: {
    optional: boolean;
    revealedValue?: string;
    revealStatus: CredentialRevealStatus;
  },
): CredentialEdit {
  if (options.optional && edit.configured && !nextValue) {
    return { ...edit, action: "clear", value: "" };
  }
  if (
    edit.configured &&
    options.revealStatus === "ready" &&
    nextValue === options.revealedValue
  ) {
    return { ...edit, action: "keep", value: "" };
  }
  return { ...edit, action: "replace", value: nextValue };
}
