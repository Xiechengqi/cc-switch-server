import { Search } from "lucide-react";

import { useI18n } from "@/lib/i18n";

interface ProviderListToolbarProps {
  query: string;
  visible: number;
  total: number;
  onQueryChange: (value: string) => void;
}

export function ProviderListToolbar({ query, visible, total, onQueryChange }: ProviderListToolbarProps) {
  const { tx } = useI18n();
  return (
    <section className="provider-list-toolbar">
      <label className="provider-list-search">
        <Search size={15} />
        <input
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder={tx("Search providers")}
        />
      </label>
      <span className="provider-list-count">{tx("{{visible}}/{{total}} providers", { visible, total })}</span>
    </section>
  );
}
