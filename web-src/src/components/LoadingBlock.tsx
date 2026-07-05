import { Loader2 } from "lucide-react";

import { useI18n } from "@/lib/i18n";

export function LoadingBlock({ label }: { label: string }) {
  const { tx } = useI18n();
  return (
    <div className="provider-empty">
      <Loader2 size={22} />
      <span>{tx(label)}</span>
    </div>
  );
}
