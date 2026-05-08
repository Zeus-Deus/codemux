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
    return (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <div className="flex flex-col items-center gap-2">
          <ImageOff className="h-8 w-8 opacity-40" />
          <span className="text-xs">Failed to load image</span>
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
