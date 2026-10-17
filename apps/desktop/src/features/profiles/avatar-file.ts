import type { Avatar } from "./profile.js";

/** The largest file the picker accepts before it is downscaled; a profile picture is not a photo library. */
const MAX_BYTES = 12 * 1024 * 1024;

/**
 * Turns a chosen file into a square 256 px JPEG data URL.
 *
 * The value is stored in the profile book on this device, not sent over IPC: an avatar is a few tens
 * of kilobytes, and a command that copied it into the app's data directory would be a second place
 * to keep the same picture.
 */
export async function fileToAvatar(file: File): Promise<Avatar> {
  if (!file.type.startsWith("image/")) throw new Error("not an image");
  if (file.size > MAX_BYTES) throw new Error("image is too large");

  const bitmap = await createImageBitmap(file);
  try {
    const side = Math.min(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas");
    canvas.width = 256;
    canvas.height = 256;
    const context = canvas.getContext("2d");
    if (context === null) throw new Error("canvas is unavailable");
    context.drawImage(
      bitmap,
      (bitmap.width - side) / 2,
      (bitmap.height - side) / 2,
      side,
      side,
      0,
      0,
      256,
      256,
    );
    return { kind: "upload", dataUrl: canvas.toDataURL("image/jpeg", 0.85) };
  } finally {
    bitmap.close();
  }
}
