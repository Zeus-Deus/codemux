/**
 * Byte counts for humans: "512 KB" / "84.0 MB" / "2.50 GB".
 *
 * 1024-based, so a figure here agrees with the resource monitor's memory
 * column and with what `du`/`df` say on the host. Precision widens with the
 * unit because a rounding error of half a megabyte matters at 3 MB and not
 * at 3 GB. Callers that want to signal an estimate add their own "~".
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;
  const TB = GB * 1024;
  if (bytes < KB) return `${Math.round(bytes)} B`;
  if (bytes < MB) return `${(bytes / KB).toFixed(0)} KB`;
  if (bytes < GB) return `${(bytes / MB).toFixed(1)} MB`;
  if (bytes < TB) return `${(bytes / GB).toFixed(2)} GB`;
  return `${(bytes / TB).toFixed(2)} TB`;
}
