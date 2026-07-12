export function isNonRetryableHttpError(error: unknown): boolean {
  if (typeof error === "object" && error !== null && "status" in error) {
    const status = Number((error as { status?: unknown }).status);
    if (status >= 400 && status <= 404) return true;
  }
  const message = error instanceof Error ? error.message : String(error);
  return /HTTP 40[0-4]/.test(message);
}
