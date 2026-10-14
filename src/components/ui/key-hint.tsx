import { Kbd } from "@/components/ui/kbd";

/**
 * One keycap plus what it does — the footer vocabulary shared by the command
 * palette and the file/content search dialogs, so all three overlays teach
 * their keys the same way.
 */
export function KeyHint({ keys, label }: { keys: string; label: string }) {
  return (
    <span className="flex items-center gap-1.5 text-label text-muted-foreground/70">
      <Kbd>{keys}</Kbd>
      {label}
    </span>
  );
}
