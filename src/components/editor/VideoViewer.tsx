import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { LoaderCircle, VideoOff } from "lucide-react";

interface Props {
  filePath: string;
}

type VideoState = "loading" | "ready" | "error";

/**
 * Plays a workspace video in-place using the webview's native media engine.
 * `convertFileSrc` resolves to Tauri's streaming asset protocol on desktop
 * and to the authenticated asset route in a remote browser session, so large
 * recordings never cross the command bridge as an in-memory byte array.
 */
export function VideoViewer({ filePath }: Props) {
  const [state, setState] = useState<VideoState>("loading");
  const src = convertFileSrc(filePath);

  if (state === "error") {
    return (
      <div className="flex flex-1 items-center justify-center bg-[#050505] px-6 text-center text-zinc-400">
        <div className="flex max-w-[280px] flex-col items-center">
          <div className="mb-3 flex size-10 items-center justify-center rounded-full border border-white/10 bg-white/[0.04]">
            <VideoOff className="size-[18px] text-zinc-500" strokeWidth={1.5} />
          </div>
          <p className="text-xs font-medium text-zinc-200">Can’t play this video</p>
          <p className="mt-1.5 text-[11px] leading-[1.55] text-zinc-500">
            The file may use a codec that isn’t available in the system webview.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div
      data-testid="video-viewer"
      className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-[#050505] p-3 sm:p-4"
    >
      {state === "loading" && (
        <div
          data-testid="video-loading"
          className="pointer-events-none absolute inset-0 flex items-center justify-center"
        >
          <LoaderCircle
            className="size-[18px] animate-spin text-zinc-600"
            strokeWidth={1.5}
            aria-label="Loading video"
          />
        </div>
      )}
      <video
        src={src}
        aria-label={`Video preview: ${filePath}`}
        autoPlay
        controls
        playsInline
        preload="metadata"
        onCanPlay={() => setState("ready")}
        onError={() => setState("error")}
        className={`max-h-full max-w-full rounded-md bg-black shadow-[0_16px_48px_rgba(0,0,0,0.45)] ring-1 ring-white/10 transition-opacity duration-200 ${
          state === "ready" ? "opacity-100" : "opacity-0"
        }`}
      >
        Your system webview does not support HTML video playback.
      </video>
    </div>
  );
}
