/**
 * A readable message for whatever a Tauri command rejected with: a string, an
 * `Error`, or the backend's `{ message, details }` command error.
 */
export function formatErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const message = (error as { message?: string }).message;
    const details = (error as { details?: string }).details;
    if (message && details) return `${message} (${details})`;
    if (message) return message;
    if (details) return details;
  }
  return String(error);
}
