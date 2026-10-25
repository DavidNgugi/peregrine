/** Bytes as a short, human unit; the library total and a transfer's size share it. */
export function formatBytes(value: number): string {
  if (value < 1024) return String(Math.round(value)) + " B";
  if (value < 1024 ** 2) return (value / 1024).toFixed(0) + " KB";
  if (value < 1024 ** 3) return (value / 1024 ** 2).toFixed(1) + " MB";
  if (value < 1024 ** 4) return (value / 1024 ** 3).toFixed(2) + " GB";
  return (value / 1024 ** 4).toFixed(2) + " TB";
}
