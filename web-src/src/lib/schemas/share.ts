import { z } from "zod";

/**
 * P8 多 app share：每个 app_type 各自独立绑定 0..1 个 provider。
 * 表单上每个 slot 都是 optional 字符串（"" 表示未绑定）。至少要选一个，否则 share
 * 创建后没有任何 app 可用，没有意义。
 */
export const createShareSchema = z.object({
  bindings: z
    .object({
      claude: z.string().trim().default(""),
      codex: z.string().trim().default(""),
      gemini: z.string().trim().default(""),
    })
    .default({ claude: "", codex: "", gemini: "" })
    .refine(
      (value) =>
        (value.claude?.length ?? 0) > 0 ||
        (value.codex?.length ?? 0) > 0 ||
        (value.gemini?.length ?? 0) > 0,
      "share.validation.providerRequired",
    )
    .refine((value) => {
      const fixedProviderIds = [value.claude, value.codex, value.gemini]
        .map((item) => item?.trim() ?? "")
        .filter((item) => item.length > 0 && item !== "__dynamic__");
      return new Set(fixedProviderIds).size === fixedProviderIds.length;
    }, "share.validation.providerDuplicate"),
  description: z
    .string()
    .trim()
    .optional()
    .transform((value) => value ?? "")
    .refine(
      (value) => value.length <= 200,
      "share.validation.descriptionTooLong",
    ),
  forSale: z.enum(["Yes", "No", "Free"]),
  saleMarketKind: z.enum(["token", "share"]).default("token"),
  tokenLimit: z.coerce
    .number()
    .int()
    .refine(
      (value) => value === -1 || value > 0,
      "share.validation.invalidTokenLimit",
    ),
  parallelLimit: z.coerce
    .number()
    .int()
    .refine(
      (value) => value === -1 || value >= 3,
      "share.validation.invalidParallelLimit",
    ),
  expiresInSecs: z.coerce.number().int().positive("share.validation.required"),
  subdomain: z
    .string()
    .trim()
    .optional()
    .transform((value) => value ?? "")
    .refine(
      (value) =>
        value.length === 0 ||
        (/^[a-z0-9](?:[a-z0-9-]{1,61}[a-z0-9])?$/.test(value) &&
          !["admin", "api", "www", "cdn-cgi"].includes(value)),
      "share.validation.invalidSubdomain",
    ),
  marketAccessMode: z.enum(["selected", "all"]).default("all"),
});

export const tunnelConfigSchema = z.object({
  domain: z
    .string()
    .trim()
    .min(1, "share.validation.required")
    .refine(
      (value) => !/[/?#]/.test(value.replace(/^https?:\/\//i, "")),
      "share.validation.invalidRouterDomain",
    ),
});

export type CreateShareFormValues = z.infer<typeof createShareSchema>;
export type TunnelConfigFormValues = z.infer<typeof tunnelConfigSchema>;
export type CreateShareFormInput = z.input<typeof createShareSchema>;
