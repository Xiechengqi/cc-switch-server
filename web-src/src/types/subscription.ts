export type CredentialStatus =
  "valid" | "expired" | "not_found" | "parse_error";

export interface QuotaTier {
  name: string;
  label?: string | null;
  utilization: number; // 0-100
  resetsAt: string | null;
  used?: number | null;
  limit?: number | null;
  unit?: string | null;
  usedValueUsd?: number | null;
  maxValueUsd?: number | null;
  planLabel?: string | null;
}

export interface ExtraUsage {
  isEnabled: boolean;
  monthlyLimit: number | null;
  usedCredits: number | null;
  utilization: number | null;
  currency: string | null;
}

export type SubscriptionExpiresKind =
  "subscription" | "billing_period" | "quota_period" | "unknown";

export interface SubscriptionInfo {
  planType?: string | null;
  planLabel?: string | null;
  expiresAt?: string | null;
  expiresSource?: string | null;
  expiresKind?: SubscriptionExpiresKind | null;
  expiryStale?: boolean | null;
  expiryCapability?:
    | "automatic"
    | "automatic_or_manual"
    | "manual_required"
    | "research_pending"
    | "not_applicable"
    | null;
  expiryAvailability?:
    | "available"
    | "upstream_not_provided"
    | "probe_unavailable"
    | "workspace_unverified"
    | null;
}

export type QuotaStatus =
  | "valid_numeric"
  | "valid_non_numeric"
  | "partial"
  | "unavailable"
  | "spending_limited";

export interface SubscriptionQuota {
  tool: string;
  credentialStatus: CredentialStatus;
  credentialMessage: string | null;
  subscription?: SubscriptionInfo | null;
  success: boolean;
  quotaStatus?: QuotaStatus | null;
  warningCodes?: string[];
  warnings?: string[];
  staleTierNames?: string[];
  tiers: QuotaTier[];
  extraUsage: ExtraUsage | null;
  error: string | null;
  queriedAt: number | null;
}
