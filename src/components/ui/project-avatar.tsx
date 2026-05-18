import { useState } from "react";
import { cn } from "@/lib/utils";
import { resolveImageUrl } from "@/lib/project-image";

interface Props {
  name: string;
  color?: string | null;
  imageUrl?: string | null;
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
  size = "lg",
  shape = "circle",
  className,
}: Props) {
  const [imgFailed, setImgFailed] = useState(false);

  const letter = (name || "?").charAt(0).toUpperCase();
  const resolved = imageUrl ? resolveImageUrl(imageUrl) : null;
  const hasImage = !!resolved?.url && !imgFailed;
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
          src={resolved!.url}
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
