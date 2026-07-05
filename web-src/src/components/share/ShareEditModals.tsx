import { useEffect, useState } from "react";
import { Store } from "lucide-react";

import { ModalFooter } from "@/components/ModalFooter";
import { SimpleModal } from "@/components/SimpleModal";
import type { AppKind, PublicShareMarket, ShareAcl, ShareRecord, StoredProvider } from "@/lib/api";
import { useI18n } from "@/lib/i18n";
import { appLabel, shareName } from "@/components/share/shareDisplay";

export interface BindingDraft {
  share: ShareRecord;
  app: AppKind;
  providerId: string;
}

export function AclModal({
  share,
  saving,
  onClose,
  onSubmit,
}: {
  share: ShareRecord;
  saving: boolean;
  onClose: () => void;
  onSubmit: (acl: ShareAcl) => void;
}) {
  const { tx } = useI18n();
  const [emails, setEmails] = useState((share.acl?.sharedWithEmails || []).join(", "));
  const [marketAccessMode, setMarketAccessMode] = useState(share.acl?.marketAccessMode || "selected");
  const [publicMarketEmail, setPublicMarketEmail] = useState(share.acl?.publicMarketEmail || "");
  return (
    <SimpleModal title="Share ACL" subtitle={shareName(share)} onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit({
            sharedWithEmails: splitList(emails),
            marketAccessMode,
            publicMarketEmail: publicMarketEmail.trim() || null,
          });
        }}
      >
        <label>
          <span>{tx("Shared emails")}</span>
          <input value={emails} onChange={(event) => setEmails(event.target.value)} />
        </label>
        <label>
          <span>{tx("Public market email")}</span>
          <input value={publicMarketEmail} onChange={(event) => setPublicMarketEmail(event.target.value)} />
        </label>
        <label>
          <span>{tx("Market mode")}</span>
          <select value={marketAccessMode} onChange={(event) => setMarketAccessMode(event.target.value)}>
            <option value="selected">{tx("selected")}</option>
            <option value="all">{tx("all")}</option>
          </select>
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Save ACL" />
      </form>
    </SimpleModal>
  );
}

export function SubdomainModal({
  share,
  saving,
  onClose,
  onSubmit,
}: {
  share: ShareRecord;
  saving: boolean;
  onClose: () => void;
  onSubmit: (subdomain: string) => void;
}) {
  const { tx } = useI18n();
  const [subdomain, setSubdomain] = useState(share.tunnelSubdomain || "");
  return (
    <SimpleModal title="Share Subdomain" subtitle={shareName(share)} onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(subdomain);
        }}
      >
        <label>
          <span>{tx("Subdomain")}</span>
          <input value={subdomain} onChange={(event) => setSubdomain(event.target.value)} />
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Save Subdomain" />
      </form>
    </SimpleModal>
  );
}

export function BindingModal({
  draft,
  providers,
  saving,
  onChange,
  onClose,
  onSubmit,
}: {
  draft: BindingDraft;
  providers: StoredProvider[];
  saving: boolean;
  onChange: (draft: BindingDraft) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  const { tx } = useI18n();
  return (
    <SimpleModal
      title="{{app}} Binding"
      titleVariables={{ app: appLabel(draft.app) }}
      subtitle="Share must be paused before binding changes are accepted."
      onClose={onClose}
    >
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <label>
          <span>{tx("Provider")}</span>
          <select value={draft.providerId} onChange={(event) => onChange({ ...draft, providerId: event.target.value })}>
            <option value="">{tx("Select provider")}</option>
            {providers.map((provider) => (
              <option key={provider.provider.id} value={provider.provider.id}>
                {provider.provider.name} ({provider.providerTypeId})
              </option>
            ))}
          </select>
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Update Binding" />
      </form>
    </SimpleModal>
  );
}

export function MarketModal({
  share,
  markets,
  marketsLoaded,
  saving,
  onLoadMarkets,
  onClose,
  onSubmit,
}: {
  share: ShareRecord;
  markets: PublicShareMarket[];
  marketsLoaded: boolean;
  saving: boolean;
  onLoadMarkets: () => void;
  onClose: () => void;
  onSubmit: (marketEmail: string) => void;
}) {
  const { tx } = useI18n();
  const shareMarkets = markets.filter((market) => market.marketKind === "share");
  const [marketEmail, setMarketEmail] = useState(shareMarkets[0]?.email || "");
  useEffect(() => {
    if (!marketEmail && shareMarkets[0]?.email) setMarketEmail(shareMarkets[0].email);
  }, [marketEmail, shareMarkets]);
  return (
    <SimpleModal title="Authorize Share Market" subtitle={shareName(share)} onClose={onClose}>
      <form
        className="modal-form-stack"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(marketEmail);
        }}
      >
        {!marketsLoaded && (
          <button className="secondary-button" type="button" onClick={onLoadMarkets}>
            <Store size={15} />
            <span>{tx("Load markets")}</span>
          </button>
        )}
        <label>
          <span>{tx("Share market")}</span>
          <select value={marketEmail} onChange={(event) => setMarketEmail(event.target.value)}>
            <option value="">{tx("Select market")}</option>
            {shareMarkets.map((market) => (
              <option key={market.id} value={market.email}>
                {market.displayName} ({market.email})
              </option>
            ))}
          </select>
        </label>
        <ModalFooter saving={saving} onClose={onClose} label="Authorize" />
      </form>
    </SimpleModal>
  );
}

function splitList(value: string): string[] {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim())
    .filter(Boolean);
}
