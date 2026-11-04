export function parseClipboardSyncPreference(value: unknown): boolean {
  if (typeof value !== "boolean") throw new Error("Invalid clipboard sync preference returned by the desktop.");
  return value;
}

export function clipboardRetryDelay(failures: number): number {
  return Math.min(30000, 1500 * 2 ** Math.min(5, Math.max(0, failures)));
}

export function canSendClipboard(content: string, snapshot: string | null, latest: string | null): boolean {
  return content.length > 0 && snapshot === latest && content !== latest;
}
