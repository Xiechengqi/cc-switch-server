import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";

import App from "./App";
import { ThemeProvider } from "./components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { UpdateProvider } from "@/contexts/UpdateContext";
import { I18nProvider } from "./lib/i18n";
import { queryClient } from "@/lib/query/queryClient";
import "./i18n";
import "./desktop-theme.css";
import "./styles.css";
import "./styles/universal.css";
import "./styles/modals.css";
import "./styles/auth-accounts.css";
import "./styles/providers.css";
import "./styles/usage.css";

try {
  const ua = navigator.userAgent || "";
  const plat = (navigator.platform || "").toLowerCase();
  if (/mac/i.test(ua) || plat.includes("mac")) {
    document.body.classList.add("is-mac");
  }
} catch {
  // ignore platform detection failures
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider defaultTheme="system" storageKey="cc-switch-theme">
        <I18nProvider>
          <UpdateProvider>
            <App />
            <Toaster />
          </UpdateProvider>
        </I18nProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
