export function isNonRetryableHttpError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /HTTP 40[0-4]/.test(message);
}
