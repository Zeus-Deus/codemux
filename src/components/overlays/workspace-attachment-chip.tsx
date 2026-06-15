import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { FileText, Image as ImageIcon, X } from "lucide-react";
import { basename } from "@/lib/path";

const IMAGE_EXTS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
  "bmp",
  "avif",
  "ico",
]);

/** Image attachments get a thumbnail; everything else a typed glyph. */
function isImagePath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return IMAGE_EXTS.has(ext);
}

/** Pasted clipboard images land in a dedicated temp dir
 *  (`codemux-clipboard-images/paste-<uuid>.<ext>`). Surfacing the raw
 *  UUID is noise — show a friendly label and keep the real path in the
 *  hover title. The dir marker is the reliable signal (cross-platform,
 *  not fooled by a user file that happens to start with `paste-`). */
function attachmentLabel(path: string, isImage: boolean): string {
  const name = basename(path);
  if (isImage && path.includes("codemux-clipboard-images")) {
    return "Pasted image";
  }
  return name;
}

interface Props {
  /** Absolute filesystem path of the attached file. */
  path: string;
  onRemove: () => void;
}

/** Chip for a staged file/image attachment in the new-workspace composer.
 *  Images render a real thumbnail (via the Tauri asset protocol); other
 *  files fall back to a typed glyph. Mirrors the footer pill vocabulary
 *  (bordered, muted) so the attachment strip reads as a sibling of the
 *  agent/model pills below it. */
export function WorkspaceAttachmentChip({ path, onRemove }: Props) {
  const [thumbErrored, setThumbErrored] = useState(false);
  const isImage = isImagePath(path);
  const showThumb = isImage && !thumbErrored;
  const label = attachmentLabel(path, isImage);

  return (
    <span
      className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 py-0.5 pl-1 pr-1 text-[11px] text-foreground"
      title={path}
    >
      {showThumb ? (
        <img
          src={convertFileSrc(path)}
          alt=""
          aria-hidden
          className="size-5 shrink-0 rounded object-cover"
          onError={() => setThumbErrored(true)}
        />
      ) : (
        <span className="flex size-5 shrink-0 items-center justify-center rounded bg-foreground/10 text-muted-foreground">
          {isImage ? (
            <ImageIcon className="h-3 w-3" />
          ) : (
            <FileText className="h-3 w-3" />
          )}
        </span>
      )}
      <span className="max-w-[160px] truncate">{label}</span>
      <button
        type="button"
        aria-label={`Remove ${label}`}
        className="ml-0.5 rounded-full p-0.5 text-muted-foreground/70 transition-colors hover:bg-foreground/10 hover:text-foreground"
        onClick={onRemove}
      >
        <X className="h-2.5 w-2.5" />
      </button>
    </span>
  );
}
