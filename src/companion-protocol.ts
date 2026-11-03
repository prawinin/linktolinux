export interface CompanionPhoto {
  id: string;
  name: string;
  size: number;
  modified: number;
}
export interface PhotoPage {
  photos: CompanionPhoto[];
  hasMore: boolean;
}

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error("Invalid phone response");
  return value as Record<string, unknown>;
}
export function parsePhotoPage(value: unknown): PhotoPage {
  const page = record(value);
  if (
    !Array.isArray(page.photos) ||
    page.photos.length > 60 ||
    typeof page.hasMore !== "boolean"
  )
    throw new Error("Invalid photo page");
  const photos = page.photos.map((item: unknown): CompanionPhoto => {
    const photo = record(item);
    if (
      typeof photo.id !== "string" ||
      !/^[1-9][0-9]{0,18}$/.test(photo.id) ||
      typeof photo.name !== "string" ||
      photo.name.length > 1024 ||
      typeof photo.size !== "number" ||
      !Number.isSafeInteger(photo.size) ||
      photo.size < 0 ||
      typeof photo.modified !== "number" ||
      !Number.isSafeInteger(photo.modified) ||
      photo.modified < 0
    )
      throw new Error("Invalid photo metadata");
    return {
      id: photo.id,
      name: photo.name,
      size: photo.size,
      modified: photo.modified,
    };
  });
  return { photos, hasMore: page.hasMore };
}
export function parseThumbnail(value: unknown): string {
  const url = record(value).dataUrl;
  if (
    typeof url !== "string" ||
    url.length > 1024 * 1024 ||
    !/^data:image\/jpeg;base64,[A-Za-z0-9+/=]+$/.test(url)
  )
    throw new Error("Invalid photo thumbnail");
  return url;
}
export function parseClipboard(value: unknown): string {
  const text = record(value).text;
  if (
    typeof text !== "string" ||
    new TextEncoder().encode(text).length > 1024 * 1024
  )
    throw new Error("Invalid clipboard text");
  return text;
}
