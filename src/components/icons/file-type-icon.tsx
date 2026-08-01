import { useMemo } from "react";
import { defaultIcon, getIcon } from "material-file-icons";
import { cn } from "@/lib/utils";

interface FileTypeIconProps {
  filename: string;
  className?: string;
}

export function FileTypeIcon({ filename, className }: FileTypeIconProps) {
  const icon = useMemo(() => getIcon(filename), [filename]);

  return (
    <span
      className={cn("shrink-0 inline-flex items-center justify-center [&>svg]:h-full [&>svg]:w-full", className)}
      data-file-icon={icon.name}
      dangerouslySetInnerHTML={{ __html: icon.svg }}
    />
  );
}

/** Whether `filename` resolves to a real language/file icon rather than the generic page. */
export function hasSpecificFileTypeIcon(filename: string): boolean {
  return getIcon(filename).name !== defaultIcon.name;
}
