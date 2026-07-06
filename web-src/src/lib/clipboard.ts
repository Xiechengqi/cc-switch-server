import { invoke } from "@tauri-apps/api/core";

import { isServerWebRuntime } from "@/lib/runtime";

async function copyWithWebFallback(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Non-secure origins (e.g. http://host:port) may expose clipboard but reject writes.
    }
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  textarea.style.top = "0";
  document.body.appendChild(textarea);
  textarea.focus();
  textarea.select();
  textarea.setSelectionRange(0, text.length);
  try {
    const copied = document.execCommand("copy");
    if (!copied) {
      throw new Error("clipboard copy is unavailable in this browser context");
    }
  } finally {
    document.body.removeChild(textarea);
  }
}

export async function copyText(text: string): Promise<void> {
  if (!isServerWebRuntime()) {
    try {
      await invoke("copy_text_to_clipboard", { text });
      return;
    } catch {
      // Desktop invoke can fail in dev; fall back to browser APIs.
    }
  }

  await copyWithWebFallback(text);
}
