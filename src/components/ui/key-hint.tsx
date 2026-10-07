/**
 * One keycap plus what it does — the footer vocabulary shared by the command
 * palette and the file/content search dialogs, so all three overlays teach
 * their keys the same way.
 */
export function KeyHint({ keys, label }: { keys: string; label: string }) {
  return (
    <span className="flex items-center gap-1.5 text-label text-muted-foreground/70">
      <kbd className="rounded-sm border border-hairline-strong px-1.5 py-px font-mono text-caption text-muted-foreground">
        {keys}
      </kbd>
      {label}
    </span>
  );
}
