import type { LayoutDensity } from "@/lib/layout-density";

/** Shared max-width shell for server top nav, main content, and full-screen panels. */
export const PAGE_SHELL_CLASS = "layout-page-shell mx-auto w-full max-w-7xl";

/** Horizontal inset aligned with the page shell. */
export const PAGE_SHELL_PADDING_X = "layout-page-padding-x px-4 sm:px-6 lg:px-8";

/** Vertical inset of the app chrome from the viewport top/bottom. */
export const APP_VIEWPORT_PADDING_Y =
  "layout-page-viewport-y py-4 sm:py-5 lg:py-6";

/** Vertical gap between the sticky header and main content. */
export const PAGE_HEADER_CONTENT_GAP = "layout-page-header-gap gap-3 sm:gap-4";

/** Root viewport wrapper for the desktop shell. */
export const LAYOUT_PAGE_VIEWPORT_CLASS =
  "layout-page-viewport flex flex-col h-screen overflow-hidden";

/** Sticky top header bar inside the shell. */
export const LAYOUT_PAGE_HEADER_CLASS = "layout-page-header";

export function isCompactDensity(density: LayoutDensity): boolean {
  return density === "compact";
}

export function shellPaddingXClass(): string {
  return PAGE_SHELL_PADDING_X;
}

export function pageViewportClass(isCompact: boolean): string {
  return isCompact
    ? cnViewport("py-1 gap-1.5")
    : cnViewport(APP_VIEWPORT_PADDING_Y, PAGE_HEADER_CONTENT_GAP);
}

function cnViewport(...parts: string[]): string {
  return parts.join(" ");
}
