// Rust managed-account ownership is authoritative. Known omissions must be
// documented explicitly; a newly added OAuth provider cannot silently disappear.
export function auditAuthCenterCoverage({ accountManagers, variantToId, registry, knownEntryGaps }) {
  const ownership = accountManagers.match(
    /fn account_credential_ownership\([\s\S]*?match provider_type \{([\s\S]*?)=> AccountCredentialOwnership::ManagedAccount,/,
  );
  if (!ownership) throw new Error("AuthCenter audit cannot resolve Rust managed-account ownership");
  const expected = new Set([...ownership[1].matchAll(/ProviderType::([A-Za-z0-9]+)/g)].map((match) => {
    const id = variantToId.get(match[1]);
    if (!id) throw new Error(`AuthCenter audit found unknown ProviderType ${match[1]}`);
    return id;
  }));
  const entries = [...registry.matchAll(/\bproviderType:\s*"([^"]+)"/g)].map((match) => match[1]);
  const errors = [];
  const registered = new Set();
  for (const type of entries) {
    if (registered.has(type)) errors.push(`AuthCenter has duplicate entry ${type}`);
    if (!expected.has(type)) errors.push(`AuthCenter entry ${type} is not a managed-account provider`);
    registered.add(type);
  }
  if (!Array.isArray(knownEntryGaps)) throw new Error("AuthCenter knownEntryGaps must be declared");
  const gaps = new Set();
  for (const gap of knownEntryGaps) {
    const type = gap?.providerType;
    if (!expected.has(type) || gap?.status !== "known_gap" || typeof gap?.reason !== "string" || !gap.reason.trim()) {
      errors.push(`AuthCenter has invalid known entry gap ${type}`);
    }
    if (gaps.has(type)) errors.push(`AuthCenter has duplicate known entry gap ${type}`);
    if (registered.has(type)) errors.push(`AuthCenter known entry gap ${type} is already implemented`);
    gaps.add(type);
  }
  for (const type of expected) {
    if (!registered.has(type) && !gaps.has(type)) errors.push(`AuthCenter is missing managed-account entry ${type}`);
  }
  return { errors, entryCount: entries.length, knownEntryGaps: [...gaps] };
}
