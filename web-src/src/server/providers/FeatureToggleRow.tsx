import type { ReactNode } from "react";
import { CircleHelp } from "lucide-react";

import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/**
 * A switch list where every row is one line. The explanations are still there — they
 * moved behind the "?" — because three permanently visible paragraphs turned a group of
 * three toggles into the tallest thing on the page, and nobody reads them twice.
 */
export function FeatureToggleList({ children }: { children: ReactNode }) {
  return (
    <TooltipProvider delayDuration={200}>
      <div className="divide-y rounded-md border border-border/60">
        {children}
      </div>
    </TooltipProvider>
  );
}

export function FeatureToggleRow({
  id,
  label,
  description,
  checked,
  onCheckedChange,
  action,
}: {
  id: string;
  label: string;
  description: string;
  checked: boolean;
  onCheckedChange: (enabled: boolean) => void;
  action?: ReactNode;
}) {
  const descriptionId = `${id}-description`;
  return (
    <div className="flex items-center gap-2 px-3 py-2">
      <Label htmlFor={id} className="min-w-0 flex-1 truncate font-normal">
        {label}
      </Label>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label={description}
            className="shrink-0 rounded-full text-muted-foreground/60 transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <CircleHelp className="h-3.5 w-3.5" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="top" className="max-w-xs leading-5">
          {description}
        </TooltipContent>
      </Tooltip>
      {/* Kept for screen readers: the tooltip text is not reachable from the switch. */}
      <span id={descriptionId} className="sr-only">
        {description}
      </span>
      {action}
      <Switch
        id={id}
        className="shrink-0"
        checked={checked}
        onCheckedChange={onCheckedChange}
        aria-describedby={descriptionId}
      />
    </div>
  );
}
