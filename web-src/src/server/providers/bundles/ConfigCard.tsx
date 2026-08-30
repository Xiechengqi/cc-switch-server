import { ChevronRight } from "lucide-react";

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";

/**
 * One collapsible block of the Provider configuration page. The page used to be six
 * equally-weighted sections with no container, so nothing could be closed and nothing
 * announced its own state; a card can say "here is what you already chose" in one line
 * while folded, which is what most of these blocks are doing most of the time.
 */
export function ConfigCard({
  id,
  title,
  icon,
  summary,
  status,
  open,
  onOpenChange,
  children,
}: {
  id: string;
  title: string;
  icon?: React.ReactNode;
  /** Collapsed-state one-liner: the current answer, not a description of the card. */
  summary?: React.ReactNode;
  status?: React.ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: React.ReactNode;
}) {
  return (
    <Collapsible
      open={open}
      onOpenChange={onOpenChange}
      className="rounded-lg border bg-card"
    >
      <div className="flex items-center gap-2 pr-3">
        <CollapsibleTrigger asChild>
          <button
            type="button"
            id={id}
            className="flex min-w-0 flex-1 items-center gap-2 rounded-lg px-3 py-3 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <ChevronRight
              className={cn(
                "h-4 w-4 shrink-0 text-muted-foreground transition-transform",
                open && "rotate-90",
              )}
            />
            {icon}
            <span className="shrink-0 text-sm font-semibold">{title}</span>
            {!open && summary ? (
              <span className="min-w-0 truncate text-xs text-muted-foreground">
                {summary}
              </span>
            ) : null}
          </button>
        </CollapsibleTrigger>
        {status ? <div className="shrink-0">{status}</div> : null}
      </div>
      <CollapsibleContent>
        <div className="border-t px-3 py-4 sm:px-4">{children}</div>
      </CollapsibleContent>
    </Collapsible>
  );
}
