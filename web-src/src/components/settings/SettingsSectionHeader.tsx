import type { ReactNode } from "react";

export function SectionHeader({ icon, title, subtitle }: { icon: ReactNode; title: string; subtitle: string }) {
  return (
    <header className="settings-card-header">
      <div className="section-title-row compact-title">
        {icon}
        <h3>{title}</h3>
      </div>
      <span>{subtitle}</span>
    </header>
  );
}
