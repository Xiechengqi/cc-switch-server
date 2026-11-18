import { useState, type ReactNode } from "react";
import { ChevronDown, LoaderCircle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

export interface ModelInputProps {
  value: string;
  onValueChange: (value: string) => void;
  id?: string;
  "aria-label"?: string;
  placeholder?: string;
  className?: string;
}

export interface ModelOption {
  id: string;
  name: string;
  value?: string;
  disabled?: boolean;
  detail?: ReactNode;
  badge?: ReactNode;
}

/** Discovery is advisory: only typing or an explicit selection changes the draft. */
export function EditableModelInput({ options, ready, isFetching, refresh, labels, children, controls,
  value, onValueChange, className, ...input }: ModelInputProps & {
  options: ModelOption[];
  ready: boolean;
  isFetching: boolean;
  refresh: () => Promise<unknown>;
  labels: { choose: string; search: string; refresh: string; noMatch: string };
  children?: ReactNode;
  controls?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const needle = search.trim().toLowerCase();
  const matches = options.filter((m) => `${m.id} ${m.name}`.toLowerCase().includes(needle));
  return <div className="min-w-0 flex-1 basis-56 space-y-1.5">
    <div className="flex items-center gap-1.5">
      <Input {...input} className={className} value={value} onChange={(event) => onValueChange(event.target.value)} />
      <Popover open={open} onOpenChange={(next) => { setOpen(next); if (!next) setSearch(""); }}>
        <PopoverTrigger asChild>
          <Button type="button" variant="outline" size="sm" disabled={!ready} aria-label={labels.choose}>
            {isFetching ? <LoaderCircle className="h-4 w-4 animate-spin" /> : <ChevronDown className="h-4 w-4" />}
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-80 max-w-[90vw] space-y-2 p-2">
          <Input aria-label={labels.search} placeholder={labels.search} value={search} onChange={(event) => setSearch(event.target.value)} />
          {controls}
          <div className="max-h-64 overflow-y-auto" role="listbox" aria-label={labels.choose} onKeyDown={(event) => {
            if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
            const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="option"]:not(:disabled)'));
            if (!buttons.length) return;
            event.preventDefault();
            const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
            const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
              : index < 0 ? (event.key === "ArrowUp" ? buttons.length - 1 : 0)
              : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
            buttons[next].focus();
          }}>
            {matches.map((m) => <button key={m.id} type="button" role="option" disabled={m.disabled}
              aria-selected={(m.value ?? m.id) === value.trim()} aria-disabled={m.disabled || undefined}
              className="block w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent focus:bg-accent disabled:opacity-50"
              onClick={() => { onValueChange(m.value ?? m.id); setOpen(false); setSearch(""); }}>
              <span className="flex items-center justify-between gap-2"><span>{m.name}</span>{m.badge}</span>
              <span className="block font-mono text-xs text-muted-foreground">{m.id}</span>
              {m.detail}
            </button>)}
            {!matches.length ? <p className="p-2 text-xs text-muted-foreground">{labels.noMatch}</p> : null}
          </div>
        </PopoverContent>
      </Popover>
      <Button type="button" variant="ghost" size="sm" disabled={!ready || isFetching} aria-label={labels.refresh} onClick={() => void refresh()}>
        <RefreshCw className="h-4 w-4" />
      </Button>
    </div>
    {children}
  </div>;
}
