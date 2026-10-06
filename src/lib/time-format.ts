import { getTimeFormat, type TimeFormat } from "@/stores/settings-store";

/**
 * Apply the user's clock preference (Settings → Appearance → Time format)
 * to `toLocaleTimeString` options.
 *
 * "system" returns the options untouched, so every call site keeps its own
 * locale-driven default. "12h" / "24h" override the hour cycle while the
 * locale still decides everything else (separators, AM/PM wording).
 * `hour12` wins over `hourCycle` in `Intl`, so each branch clears the other.
 */
export function withTimeFormat(
  options: Intl.DateTimeFormatOptions = {},
  format: TimeFormat = getTimeFormat(),
): Intl.DateTimeFormatOptions {
  if (format === "system") return options;
  const { hour12: _hour12, hourCycle: _hourCycle, ...rest } = options;
  return format === "12h"
    ? { ...rest, hour12: true }
    : { ...rest, hourCycle: "h23" };
}
