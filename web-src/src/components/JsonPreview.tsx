interface JsonPreviewProps {
  value: unknown;
  redact?: boolean;
}

export function JsonPreview({ value, redact = false }: JsonPreviewProps) {
  return <pre className="json-preview">{JSON.stringify(redact ? redactSecrets(value) : value, null, 2)}</pre>;
}

function redactSecrets(value: unknown): unknown {
  if (Array.isArray(value)) {
    if (value.length === 2 && typeof value[0] === "string" && isSecretKey(value[0])) {
      return [value[0], value[1] == null || value[1] === "" ? value[1] : "[REDACTED]"];
    }
    return value.map(redactSecrets);
  }
  if (!value || typeof value !== "object") return value;
  const redacted: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
    redacted[key] = isSecretKey(key)
      ? item == null || item === ""
        ? item
        : "[REDACTED]"
      : redactSecrets(item);
  }
  return redacted;
}

function isSecretKey(key: string): boolean {
  const lower = key.toLowerCase();
  return (
    lower.includes("token") ||
    lower.includes("secret") ||
    lower.includes("apikey") ||
    lower.includes("api_key") ||
    lower === "code" ||
    lower.includes("codeverifier") ||
    lower.includes("code_verifier") ||
    lower === "authorization"
  );
}
