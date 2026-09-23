import {
  addonCode,
  addonMessage,
  type AddonAccess,
  type AddonCredentialState,
  type AddonListing,
  type AddonManifest,
  type AddonSource,
} from "@/lib/addons/types";

export const PERMISSION_LABELS: Record<string, string> = {
  "workspace.read": "Read local project metadata",
  "git.read": "Read a bounded Git summary",
  "composer.append": "Append text after your interaction",
  "external.open": "Open HTTPS links after your interaction",
};

const PLATFORM_LABELS: Record<string, string> = {
  "linux-x64": "Linux x64",
  "windows-x64": "Windows x64",
};
export const platformLabel = (platform: string) =>
  PLATFORM_LABELS[platform] ?? platform;

export const TIER_LABELS = { official: "Official", community: "Community" };

/** The reviewed identity to show for a source: never the manifest's author. */
export function sourceIdentity(
  source: AddonSource,
  listing?: AddonListing | null,
): string {
  if (source.kind === "local") return "Local / unverified";
  return `Published by ${listing?.publisher ?? source.publisher}`;
}

export function manifestAccess(manifest: AddonManifest): AddonAccess {
  return {
    permissions: manifest.permissions,
    http: manifest.http.map(({ origin, methods }) => ({ origin, methods })),
    credentials: manifest.credentials.map(({ id, label, origin }) => ({
      id,
      label,
      origin,
    })),
  };
}
export const hasAccess = (access: AddonAccess | undefined) =>
  !!access &&
  (access.permissions.length > 0 ||
    access.http.length > 0 ||
    access.credentials.length > 0);

/** "1 command", "2 commands". */
export const count = (n: number, singular: string, plural = `${singular}s`) =>
  `${n} ${n === 1 ? singular : plural}`;

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export const CREDENTIAL_STATES: Record<
  AddonCredentialState,
  { label: string; detail: (origin: string) => string }
> = {
  "not-configured": {
    label: "Not configured",
    detail: (origin) =>
      `Requests to ${origin} are sent without a credential.`,
  },
  saved: {
    label: "Stored in the system credential store",
    detail: (origin) => `Attached only to requests to ${origin}.`,
  },
  "session-only": {
    label: "Stored for this session only",
    detail: (origin) =>
      `Attached only to requests to ${origin} until CodeMux quits.`,
  },
  "cleanup-pending": {
    label: "Removal pending",
    detail: (origin) =>
      `No longer attached to requests to ${origin}. Removing it from the system credential store is still queued; use Retry cleanup.`,
  },
};

/** A rejection shown to the user: a short, specific title plus the host's message. */
export interface AddonProblem {
  title: string | null;
  message: string;
}
/**
 * Names the reason behind a catalog, link or update rejection. The host's
 * message stays the detail; the title tells unsupported platform, API,
 * blocked, unlisted and unreachable-catalog cases apart at a glance.
 */
export function addonProblem(error: unknown): AddonProblem {
  const message = addonMessage(error);
  const code = addonCode(error);
  const title =
    code === "INCOMPATIBLE_API"
      ? /not available for|is available for|unsupported on this platform/.test(
          message,
        )
        ? "Not available for this device"
        : /add-on API|plugin API/.test(message)
          ? "Incompatible add-on API"
          : "Incompatible release"
      : code === "PERMISSION_DENIED" && /blocked/i.test(message)
        ? "Release blocked"
        : code === "INVALID_MESSAGE"
          ? /not listed/.test(message)
            ? "Not in the catalog"
            : /already installed/.test(message)
              ? "Already installed"
              : /Downgrades/.test(message)
                ? "Older release"
                : /install link|catalog ID|semantic version|stable release/.test(
                      message,
                    )
                  ? "Not a valid install link or ID"
                  : null
          : code === "NETWORK_DENIED" || code === "TIMEOUT"
            ? "Network unavailable"
            : null;
  return { title, message };
}
