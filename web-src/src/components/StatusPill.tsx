import { ReactNode } from "react";

export type StatusPillTone = "success" | "warning" | "danger";

export function StatusPill({ children, tone }: { children: ReactNode; tone: StatusPillTone }) {
  return <span className={`status-pill ${tone}`}>{children}</span>;
}
