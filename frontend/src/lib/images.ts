/** Images pasted or dropped into the chat composer, prepared for the model. */
import type { ImageAttachment } from "@/lib/ipc";

/** Longest side sent to the model: larger images cost more tokens without
 *  helping (Anthropic recommends ≤1568 px). */
const MAX_SIDE = 1568;
/** Keep the original bytes below this size (and when no resize is needed). */
const KEEP_BYTES = 3_500_000;
const TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

function toBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1] ?? "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

/** Scale the image down if needed and encode it (PNG stays PNG when small
 *  enough, otherwise JPEG). */
export async function prepareImage(file: Blob): Promise<ImageAttachment> {
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, MAX_SIDE / Math.max(bitmap.width, bitmap.height));
  if (scale === 1 && file.size <= KEEP_BYTES && TYPES.includes(file.type)) {
    bitmap.close();
    return { mime: file.type, data: await toBase64(file) };
  }
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(bitmap.width * scale);
  canvas.height = Math.round(bitmap.height * scale);
  canvas.getContext("2d")!.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  bitmap.close();
  const encode = (type: string, quality?: number) =>
    new Promise<Blob>((resolve, reject) =>
      canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("could not encode the image"))), type, quality),
    );
  let blob = file.type === "image/png" ? await encode("image/png") : null;
  if (!blob || blob.size > KEEP_BYTES) blob = await encode("image/jpeg", 0.85);
  return { mime: blob.type, data: await toBase64(blob) };
}

export const imageUrl = (img: ImageAttachment) => `data:${img.mime};base64,${img.data}`;
