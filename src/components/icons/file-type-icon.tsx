import { useMemo } from "react";
import { getIcon } from "material-file-icons";
import { cn } from "@/lib/utils";

interface FileTypeIconProps {
  filename: string;
  className?: string;
}

export function FileTypeIcon({ filename, className }: FileTypeIconProps) {
  const svg = useMemo(() => getIcon(filename).svg, [filename]);

  return (
    <span
      className={cn("shrink-0 inline-flex items-center justify-center [&>svg]:h-full [&>svg]:w-full", className)}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
