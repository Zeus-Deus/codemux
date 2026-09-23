import { useState } from "react";
import { ShieldCheck } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { AddonReview } from "@/lib/addons/types";
import {
  AccessSection,
  AddonDialogContent,
  Capabilities,
  Contributions,
  NetworkNotes,
  ProblemAlert,
} from "./addon-parts";
import {
  TIER_LABELS,
  formatBytes,
  hasAccess,
  manifestAccess,
  platformLabel,
  sourceIdentity,
  type AddonProblem,
} from "./addon-presentation";

export interface ReviewChoice {
  enable: boolean;
  replaceSource: boolean;
  restoreData: boolean;
}

/** A same-source update keeps its grant identity; everything else installs. */
const isUpdateReview = (review: AddonReview) =>
  !!review.installed && !review.replacesSource;

export function AddonReviewDialog({
  review,
  busy,
  paused,
  problem,
  ended,
  returnFocus,
  onDismiss,
  onAccept,
}: {
  review: AddonReview | null;
  busy: boolean;
  paused: boolean;
  problem: AddonProblem | null;
  /** The host uses up a review on accept, even when the install then fails. */
  ended: boolean;
  returnFocus: HTMLElement | null;
  onDismiss: () => void;
  onAccept: (choice: ReviewChoice) => void;
}) {
  return (
    <Dialog
      open={review !== null}
      onOpenChange={(open) => {
        if (!open && !busy) onDismiss();
      }}
    >
      <AddonDialogContent
        className="max-h-[85vh] overflow-auto sm:max-w-lg"
        returnFocus={returnFocus}
      >
        <DialogHeader>
          <DialogTitle>Review {review?.manifest.name}</DialogTitle>
          <DialogDescription>{review && summary(review)}</DialogDescription>
        </DialogHeader>
        {review && (
          <ReviewBody
            key={review.token}
            review={review}
            busy={busy}
            paused={paused}
            problem={problem}
            ended={ended}
            onAccept={onAccept}
          />
        )}
      </AddonDialogContent>
    </Dialog>
  );
}

function summary(review: AddonReview) {
  const identity = sourceIdentity(review.source, review.catalog);
  const version = review.manifest.version;
  if (review.installed && !review.replacesSource)
    return `Update ${review.installed.version} → ${version} · ${identity}`;
  if (review.installed)
    return `Version ${version} · ${identity} · replaces ${review.installed.version} from another source`;
  return `Version ${version} · ${identity}`;
}

function ReviewBody({
  review,
  busy,
  paused,
  problem,
  ended,
  onAccept,
}: {
  review: AddonReview;
  busy: boolean;
  paused: boolean;
  problem: AddonProblem | null;
  ended: boolean;
  onAccept: (choice: ReviewChoice) => void;
}) {
  const [replace, setReplace] = useState(false);
  const [restoreData, setRestoreData] = useState(false);
  const { manifest } = review;
  const update = isUpdateReview(review);
  const added = review.added ?? manifestAccess(manifest);
  const blocked = busy || ended || (review.replacesSource && !replace);
  return (
    <>
      <p className="text-body">{manifest.description}</p>
      {review.development && (
        // The host starts watching only on an accept that enables the add-on
        // and stops on disable; a later Enable does not watch the file again.
        <p className="rounded-sm border p-3 text-body">
          Development package.{" "}
          {update ? (
            <>
              CodeMux keeps watching this selected file for validated local
              rebuilds until you disable the add-on or turn off Developer mode.
            </>
          ) : (
            <>
              Install &amp; enable also watches this selected file for
              validated local rebuilds until you disable the add-on or turn off
              Developer mode. Installing it disabled, or enabling it later,
              does not watch the file.
            </>
          )}{" "}
          Permission changes still need review.
        </p>
      )}
      <div className="grid gap-1 text-body">
        {review.source.kind === "catalog" ? (
          <>
            <span className="flex flex-wrap items-center gap-1.5">
              {review.catalog?.tier === "official" && (
                <ShieldCheck
                  className="size-4 text-muted-foreground"
                  aria-hidden
                />
              )}
              {sourceIdentity(review.source, review.catalog)}
              {review.catalog?.tier && (
                <Badge variant="outline">
                  {TIER_LABELS[review.catalog.tier]}
                </Badge>
              )}
            </span>
            <span className="text-label text-muted-foreground">
              Catalog-reviewed listing ·{" "}
              <span className="break-all">
                {review.catalog?.repository ?? review.source.repository}
              </span>
            </span>
          </>
        ) : (
          <>
            <span>Local / unverified</span>
            <span className="text-label text-muted-foreground">
              Not reviewed by the CodeMux catalog. Install packages only from
              sources you trust.
            </span>
          </>
        )}
        <span className="text-label text-muted-foreground">
          Author (as stated by the package): {manifest.author.name} · License{" "}
          {manifest.license}
        </span>
        <span className="text-label text-muted-foreground">
          Add-on API {manifest.api} ·{" "}
          {manifest.platforms.map(platformLabel).join(", ")} ·{" "}
          {formatBytes(review.compressedBytes)}
        </span>
      </div>
      {update ? (
        <div className="space-y-3">
          {review.expandsAccess ? (
            <AccessSection title="New access" access={added} tone="new">
              <p className="text-label text-muted-foreground">
                Not granted yet. Updating grants this access.
              </p>
            </AccessSection>
          ) : (
            <p role="note" className="text-body">
              <span className="font-medium">No new access.</span> This update
              uses only access you already granted.
            </p>
          )}
          {hasAccess(review.removed) && (
            <AccessSection
              title="Removed access"
              access={review.removed!}
              tone="removed"
            >
              <p className="text-label">Revoked as soon as you update.</p>
            </AccessSection>
          )}
          <Contributions manifest={manifest} />
          <NetworkNotes manifest={manifest} />
        </div>
      ) : (
        <div className="space-y-3">
          <Capabilities manifest={manifest} />
          {hasAccess(review.removed) && (
            <AccessSection
              title="Removed access"
              access={review.removed!}
              tone="removed"
            >
              <p className="text-label">
                The replaced installation had this access. It is not carried
                over.
              </p>
            </AccessSection>
          )}
        </div>
      )}
      <code className="break-all text-label">SHA-256 {review.digest}</code>
      {review.replacesSource && (
        <label className="flex items-start gap-2 text-body">
          <input
            type="checkbox"
            checked={replace}
            onChange={(e) => setReplace(e.target.checked)}
          />
          Replace the existing source with this package. Its previous grants
          and private data will not carry over.
        </label>
      )}
      {review.retainedData && (
        <label className="flex items-start gap-2 text-body">
          <input
            type="checkbox"
            checked={restoreData}
            onChange={(e) => setRestoreData(e.target.checked)}
          />
          Restore private data retained from version{" "}
          {review.retainedData.version} of this same source. Credentials are
          not restored.
        </label>
      )}
      {paused && (
        <p role="note" className="rounded-lg border bg-muted/40 p-3 text-body">
          Add-ons are paused. CodeMux saves this choice and starts nothing until
          you resume add-ons.
        </p>
      )}
      <ProblemAlert problem={problem} />
      {ended && (
        <p role="status" className="text-body">
          This review has ended. To try again, close it and review the package
          again.
        </p>
      )}
      <DialogFooter>
        {update ? (
          <Button
            disabled={busy || ended}
            onClick={() =>
              onAccept({
                // Same-source updates keep the add-on's current enablement.
                enable: review.installed!.desiredEnabled,
                replaceSource: false,
                restoreData,
              })
            }
          >
            Update to {manifest.version}
          </Button>
        ) : (
          <>
            <Button
              variant="outline"
              disabled={blocked}
              onClick={() =>
                onAccept({ enable: false, replaceSource: replace, restoreData })
              }
            >
              Install
            </Button>
            <Button
              disabled={blocked}
              onClick={() =>
                onAccept({ enable: true, replaceSource: replace, restoreData })
              }
            >
              Install &amp; enable
            </Button>
          </>
        )}
      </DialogFooter>
    </>
  );
}
