import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ImageOff } from "lucide-react";

interface Props {
  filePath: string;
}

/**
 * Renders an image file (png/jpg/gif/webp/svg/etc.) directly when the
 * user opens it from the file tree. Uses Tauri's asset protocol via
 * `convertFileSrc` so the local path becomes a webview-safe URL.
 *
 * Image protocol must be enabled in `src-tauri/tauri.conf.json` under
 * `app.security.assetProtocol`.
 */
export function ImageViewer({ filePath }: Props) {
  const [errored, setErrored] = useState(false);
  const src = convertFileSrc(filePath);

  if (errored) {
    // Mirror VideoViewer's failure card, and always show the resolved
    // path: "failed to load" without it is undiagnosable, and the most
    // common cause is a chat link that resolved somewhere the file isn't.
    return (
      <div className="flex flex-1 items-center justify-center bg-[var(--background)] px-6 text-center text-muted-foreground">
        <div className="flex max-w-[320px] flex-col items-center">
          <div className="mb-3 flex size-10 items-center justify-center rounded-full border border-border bg-muted/40">
            <ImageOff
              className="size-[18px] text-muted-foreground/60"
              strokeWidth={1.5}
            />
          </div>
          <p className="text-xs font-medium text-foreground">
            Failed to load image
          </p>
          <p className="mt-1.5 break-all font-mono text-[11px] leading-[1.55] text-muted-foreground">
            {filePath}
          </p>
          <p className="mt-1.5 text-[11px] leading-[1.55] text-muted-foreground">
            The file may have been moved or deleted, or the format isn’t
            supported by the system webview.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-1 min-h-0 items-center justify-center overflow-auto bg-[var(--background)] p-4">
      <img
        src={src}
        alt={filePath}
        className="max-w-full max-h-full object-contain"
        onError={() => setErrored(true)}
      />
    </div>
  );
}
