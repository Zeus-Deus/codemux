// Human-readable formatting for the resource monitor.

/** Resident bytes → "512 KB" / "84.1 MB" / "1.23 GB". */
export function formatMemory(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB";
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/** CPU percentage → "12.4%" (can exceed 100% across multiple cores). */
export function formatCpu(percent: number): string {
  if (!Number.isFinite(percent) || percent < 0) return "0.0%";
  return `${percent.toFixed(1)}%`;
}

/** Whole-number percentage → "37%". */
export function formatPercent(value: number): string {
  if (!Number.isFinite(value) || value < 0) return "0%";
  return `${value.toFixed(0)}%`;
}
