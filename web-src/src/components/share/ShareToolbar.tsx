import { Search, SlidersHorizontal } from "lucide-react";

import { useI18n } from "@/lib/i18n";

export type ShareFilter = "all" | "active" | "paused" | "expired" | "exhausted" | "sale";
export type ShareSort = "createdAtDesc" | "expiresAtAsc" | "tokensUsedDesc" | "nameAsc";

interface ShareToolbarProps {
  query: string;
  filter: ShareFilter;
  sort: ShareSort;
  total: number;
  visible: number;
  onQueryChange: (value: string) => void;
  onFilterChange: (value: ShareFilter) => void;
  onSortChange: (value: ShareSort) => void;
}

export function ShareToolbar({
  query,
  filter,
  sort,
  total,
  visible,
  onQueryChange,
  onFilterChange,
  onSortChange,
}: ShareToolbarProps) {
  const { tx } = useI18n();
  const filters: Array<{ id: ShareFilter; label: string }> = [
    { id: "all", label: "all" },
    { id: "active", label: "active" },
    { id: "paused", label: "paused" },
    { id: "expired", label: "expired" },
    { id: "exhausted", label: "exhausted" },
    { id: "sale", label: "for sale" },
  ];
  const sortOptions: Array<{ id: ShareSort; label: string }> = [
    { id: "createdAtDesc", label: "Created time desc" },
    { id: "expiresAtAsc", label: "Expires time asc" },
    { id: "tokensUsedDesc", label: "Tokens used desc" },
    { id: "nameAsc", label: "Name asc" },
  ];
  return (
    <section className="share-toolbar">
      <label className="share-search">
        <Search size={15} />
        <input
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder={tx("Search shares")}
        />
      </label>
      <label className="share-select">
        <SlidersHorizontal size={14} />
        <select
          value={filter}
          onChange={(event) => onFilterChange(event.target.value as ShareFilter)}
          aria-label={tx("Share filters")}
        >
          {filters.map((item) => (
            <option key={item.id} value={item.id}>
              {tx(item.label)}
            </option>
          ))}
        </select>
      </label>
      <label className="share-select">
        <SlidersHorizontal size={14} />
        <select value={sort} onChange={(event) => onSortChange(event.target.value as ShareSort)}>
          {sortOptions.map((item) => (
            <option key={item.id} value={item.id}>
              {tx(item.label)}
            </option>
          ))}
        </select>
      </label>
      <span className="share-filter-count">{tx("{{visible}}/{{total}} shares", { visible, total })}</span>
    </section>
  );
}
