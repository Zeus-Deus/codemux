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
 * Titles for catalog, link and update rejections, by host error code and
 * message; the first match wins and `null` matches any message. The host's
 * message stays the detail; the title tells an unsupported device or API,
 * a blocked or unlisted release and an unreachable catalog apart at a glance.
 */
const PROBLEM_TITLES: [code: string, message: RegExp | null, title: string][] = [
  // The catalog says a release is "not available for <platform>"; the host
  // words a device without add-on support three ways.
  [
    "INCOMPATIBLE_API",
    /not available for|is available for|(not supported|unsupported) on this platform|platform is not supported/,
    "Not available for this device",
  ],
  ["INCOMPATIBLE_API", /add-on API|plugin API/, "Incompatible add-on API"],
  ["INCOMPATIBLE_API", null, "Incompatible release"],
  ["PERMISSION_DENIED", /blocked/i, "Release blocked"],
  ["INVALID_MESSAGE", /not listed|has no releases/, "Not in the catalog"],
  ["INVALID_MESSAGE", /already installed/, "Already installed"],
  ["INVALID_MESSAGE", /Downgrades/, "Older release"],
  [
    "INVALID_MESSAGE",
    /install link|catalog ID|semantic version|stable release/,
    "Not a valid install link or ID",
  ],
  ["NETWORK_DENIED", null, "Network unavailable"],
  ["TIMEOUT", null, "Network unavailable"],
];
export function addonProblem(error: unknown): AddonProblem {
  const message = addonMessage(error);
  const code = addonCode(error);
  const rule = PROBLEM_TITLES.find(
    ([c, pattern]) => c === code && (!pattern || pattern.test(message)),
  );
  return { title: rule?.[2] ?? null, message };
}
