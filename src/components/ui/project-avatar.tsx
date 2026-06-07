import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import { resolveImageUrl } from "@/lib/project-image";

interface Props {
  name: string;
  color?: string | null;
  imageUrl?: string | null;
  /**
   * Token appended to derived favicon URLs to bust the WebView image cache.
   * Change it when the user re-saves/re-opens the picker so a site's updated
   * favicon is actually re-fetched instead of served stale from cache.
   */
  cacheBust?: string | number | null;
  size?: "sm" | "md" | "lg";
  shape?: "circle" | "square";
  className?: string;
}

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

const SIZE_CLASSES = {
  sm: "size-3.5 text-[8px] border",
  md: "size-5 text-[10px] border",
  lg: "size-6 text-xs border-[1.5px]",
} as const;

export function ProjectAvatar({
  name,
  color,
  imageUrl,
  cacheBust,
  size = "lg",
  shape = "circle",
  className,
}: Props) {
  const [imgFailed, setImgFailed] = useState(false);

  const letter = (name || "?").charAt(0).toUpperCase();
  const resolved = imageUrl ? resolveImageUrl(imageUrl, cacheBust) : null;
  const resolvedUrl = resolved?.url ?? "";

  // Retry the image whenever the resolved URL changes (new image or a fresh
  // cache-bust token) so a previous load failure doesn't pin us to the letter.
  useEffect(() => {
    setImgFailed(false);
  }, [resolvedUrl]);

  const hasImage = !!resolvedUrl && !imgFailed;
  const hasColor = !!color;
  const shapeClass = shape === "circle" ? "rounded-full" : "rounded";

  if (hasImage) {
    return (
      <div
        className={cn(
          "flex items-center justify-center shrink-0 overflow-hidden bg-muted",
          SIZE_CLASSES[size],
          shapeClass,
          "border-border",
          className,
        )}
      >
        <img
          src={resolvedUrl}
          alt=""
          className="h-full w-full object-cover"
          onError={() => setImgFailed(true)}
          draggable={false}
        />
      </div>
    );
  }

  return (
    <div
      className={cn(
        "flex items-center justify-center shrink-0 font-medium select-none",
        SIZE_CLASSES[size],
        shapeClass,
        !hasColor && "bg-muted text-muted-foreground border-border",
        className,
      )}
      style={
        hasColor
          ? {
              borderColor: hexToRgba(color!, 0.5),
              backgroundColor: hexToRgba(color!, 0.12),
              color: color!,
            }
          : undefined
      }
    >
      {letter}
    </div>
  );
}
