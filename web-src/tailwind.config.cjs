/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: ["selector", ".dark"],
  theme: {
    extend: {
      colors: {
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        panel: "hsl(var(--panel))",
        border: "hsl(var(--border))",
        muted: "hsl(var(--muted))",
        subtle: "hsl(var(--subtle))",
        primary: "hsl(var(--primary))",
        success: "hsl(var(--success))",
        warning: "hsl(var(--warning))",
        danger: "hsl(var(--danger))",
      },
      fontFamily: {
        sans: [
          "-apple-system",
          "BlinkMacSystemFont",
          "\"Segoe UI\"",
          "Roboto",
          "\"Helvetica Neue\"",
          "Arial",
          "sans-serif",
        ],
        mono: [
          "ui-monospace",
          "SFMono-Regular",
          "\"SF Mono\"",
          "Consolas",
          "\"Liberation Mono\"",
          "Menlo",
          "monospace",
        ],
      },
      borderRadius: {
        sm: "0.375rem",
        md: "0.5rem",
      },
    },
  },
  plugins: [],
};
