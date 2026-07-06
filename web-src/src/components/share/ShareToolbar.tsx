import { useTranslation } from "react-i18next";
import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

interface ShareToolbarProps {
  search: string;
  onSearchChange: (value: string) => void;
  statusFilter: string;
  onStatusFilterChange: (value: string) => void;
  sortBy: string;
  onSortByChange: (value: string) => void;
}

export function ShareToolbar({
  search,
  onSearchChange,
  statusFilter,
  onStatusFilterChange,
  sortBy,
  onSortByChange,
}: ShareToolbarProps) {
  const { t } = useTranslation();

  return (
    <div className="grid gap-3 lg:grid-cols-[minmax(0,1.5fr)_repeat(2,minmax(180px,1fr))]">
      <div className="relative">
        <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={search}
          onChange={(event) => onSearchChange(event.target.value)}
          className="pl-9"
          placeholder={t("share.search")}
        />
      </div>
      <Select value={statusFilter} onValueChange={onStatusFilterChange}>
        <SelectTrigger>
          <SelectValue placeholder={t("share.filter.status")} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">{t("share.filter.all")}</SelectItem>
          <SelectItem value="active">{t("share.statuses.active")}</SelectItem>
          <SelectItem value="paused">{t("share.statuses.paused")}</SelectItem>
          <SelectItem value="expired">{t("share.statuses.expired")}</SelectItem>
          <SelectItem value="exhausted">
            {t("share.statuses.exhausted")}
          </SelectItem>
        </SelectContent>
      </Select>
      <Select value={sortBy} onValueChange={onSortByChange}>
        <SelectTrigger>
          <SelectValue placeholder={t("share.sort")} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="createdAtDesc">
            {t("share.sortOptions.createdAtDesc")}
          </SelectItem>
          <SelectItem value="expiresAtAsc">
            {t("share.sortOptions.expiresAtAsc")}
          </SelectItem>
          <SelectItem value="tokensUsedDesc">
            {t("share.sortOptions.tokensUsedDesc")}
          </SelectItem>
          <SelectItem value="nameAsc">
            {t("share.sortOptions.nameAsc")}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}
