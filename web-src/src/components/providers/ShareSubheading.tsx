import type { ReactNode } from "react";

/**
 * The middle of three heading levels in Share settings: Section title above,
 * field labels below. Every block inside "Remote sharing" (access, Share
 * limits, users) starts with one, so the blocks read as siblings.
 */
export function ShareSubheading({ id, title, description, action }: {
  id?: string;
  title: string;
  description?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-2">
      <div className="min-w-0 space-y-0.5">
        <h4 id={id} className="text-sm font-semibold leading-6 text-foreground">{title}</h4>
        {description ? <p className="text-xs leading-5 text-muted-foreground">{description}</p> : null}
      </div>
      {action ? <div className="flex flex-wrap items-center justify-end gap-2">{action}</div> : null}
    </div>
  );
}
