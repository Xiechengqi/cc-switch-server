import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";

import App from "./App";
import { LayoutDensityProvider } from "./components/LayoutDensityProvider";
import { ThemeProvider } from "./components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { I18nProvider } from "./lib/i18n";
import { queryClient } from "@/lib/query/queryClient";
import { detectLayoutDensity, applyLayoutDensityClass } from "./lib/layout-density";
import "./i18n";
import "./desktop-theme.css";
import "./styles.css";
import "./styles/modals.css";
import "./styles/auth-accounts.css";
import "./styles/providers.css";
import "./styles/usage.css";
import "./styles/density.css";

try {
  const ua = navigator.userAgent || "";
  const plat = (navigator.platform || "").toLowerCase();
  if (/mac/i.test(ua) || plat.includes("mac")) {
    document.body.classList.add("is-mac");
  }
} catch {
  // ignore platform detection failures
}

applyLayoutDensityClass(detectLayoutDensity());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider defaultTheme="light" storageKey="cc-switch-theme">
        <I18nProvider>
            <LayoutDensityProvider>
              <App />
            </LayoutDensityProvider>
            <Toaster />
        </I18nProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
