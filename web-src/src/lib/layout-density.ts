import { isRemoteWebMode } from "@/lib/api/auth";

/** Reserved for a future mobile viewport pass; not detected in embed-compact phase. */
export type LayoutDensity = "comfortable" | "compact";

export const DENSITY_COMPACT_CLASS = "density-compact";

export function isInEmbedFrame(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.self !== window.top;
  } catch {
    return true;
  }
}

export function detectLayoutDensity(): LayoutDensity {
  if (typeof window === "undefined") return "comfortable";

  const params = new URLSearchParams(window.location.search);
  const explicit = params.get("density") ?? params.get("embed");
  if (explicit === "comfortable") return "comfortable";
  if (explicit === "compact") return "compact";

  if (isRemoteWebMode() && isInEmbedFrame()) {
    return "compact";
  }

  return "comfortable";
}

export function applyLayoutDensityClass(density: LayoutDensity): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.classList.toggle(DENSITY_COMPACT_CLASS, density === "compact");
}
