export const MAX_FILE_BYTES = 8 * 1024 ** 3;
export type FileTransferStatus = "queued" | "waiting" | "transferring" | "complete" | "failed" | "cancelled";
export interface FileTransferView {
  id: string;
  deviceId: string;
  name: string;
  direction: "send" | "receive";
  size: number | null;
  transferred: number;
  status: FileTransferStatus;
  error: string | null;
  savedPath: string | null;
  startedAt: number;
  finishedAt: number | null;
}
const statuses = new Set<FileTransferStatus>(["queued", "waiting", "transferring", "complete", "failed", "cancelled"]);
function counter(value: unknown, maximum: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}
function nullableText(value: unknown, maximum: number): value is string | null {
  return value === null || (typeof value === "string" && value.length <= maximum);
}
export function parseTransfers(value: unknown): FileTransferView[] {
  if (!Array.isArray(value) || value.length > 100) throw new Error("Invalid transfer history");
  return value.map((entry: unknown) => {
    if (typeof entry !== "object" || entry === null) throw new Error("Invalid file transfer");
    const item = entry as Record<string, unknown>;
    if (typeof item.id !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(item.id)
      || typeof item.deviceId !== "string" || item.deviceId.length < 1 || item.deviceId.length > 128
      || typeof item.name !== "string" || !item.name.trim() || item.name === "." || item.name === ".."
      || /[/\\\u0000-\u001f\u007f]/.test(item.name) || new TextEncoder().encode(item.name).length > 240
      || (item.direction !== "send" && item.direction !== "receive")
      || (item.size !== null && !counter(item.size, MAX_FILE_BYTES))
      || !counter(item.transferred, MAX_FILE_BYTES)
      || typeof item.status !== "string" || !statuses.has(item.status as FileTransferStatus)
      || !nullableText(item.error, 1000) || !nullableText(item.savedPath, 8192)
      || !counter(item.startedAt, Number.MAX_SAFE_INTEGER)
      || (item.finishedAt !== null && !counter(item.finishedAt, Number.MAX_SAFE_INTEGER))) {
      throw new Error("Invalid file transfer details");
    }
    return item as unknown as FileTransferView;
  });
}
export function validateFileSelection(value: unknown): string[] {
  if (!Array.isArray(value) || value.length > 16
    || value.some((path: unknown) => typeof path !== "string" || !path.startsWith("/") || path.length > 8192 || /[\u0000-\u001f\u007f]/.test(path))) {
    throw new Error("Choose up to 16 files at a time");
  }
  return [...new Set(value as string[])];
}
export function transferProgress(item: FileTransferView): number | null {
  if (item.status === "complete") return 100;
  if (item.size === null) return null;
  if (item.size === 0) return 0;
  return Math.min(100, Math.max(0, Math.round(item.transferred / item.size * 100)));
}
export function formatTransferBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB"];
  let amount = bytes;
  let index = 0;
  while (amount >= 1024 && index < units.length - 1) { amount /= 1024; index++; }
  return amount.toLocaleString(undefined, {maximumFractionDigits: index === 0 || Number.isInteger(amount) ? 0 : 1}) + " " + units[index];
}
export function transferLabel(item: FileTransferView): string {
  switch (item.status) {
    case "queued": return "Queued";
    case "waiting": return "Waiting for the other device";
    case "transferring": return item.direction === "send" ? "Sending" : "Receiving";
    case "complete": return item.direction === "send" ? "Sent" : "Received";
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
  }
}
export function activeTransfer(item: FileTransferView): boolean {
  return item.status === "queued" || item.status === "waiting" || item.status === "transferring";
}

export async function withFileDeadline<T>(operation: Promise<T>, milliseconds = 10000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error("The desktop file operation timed out. Check transfer status before trying again.")), milliseconds);
  });
  try { return await Promise.race([operation, deadline]); }
  finally { clearTimeout(timer); }
}
export function filePollDelay(failures: number): number {
  return Math.min(15000, 1000 * 2 ** Math.min(failures, 4));
}
