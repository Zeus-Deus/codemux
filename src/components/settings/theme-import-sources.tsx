import { Check, Code2, FileJson, Square } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Where an imported theme came from.
 *
 * This is a *source* choice, not a format choice — the parser sniffs the
 * format on its own. What it changes is how you get the text into the modal:
 * a VS Code theme is fetched by name from the Marketplace, the other two are
 * pasted or dropped. Naming the sources is the point: "import a theme" is a
 * question about a file, "where is it from?" is a question you can answer.
 */
export type ThemeImportSourceKind = "vscode" | "shadcn" | "file";

const SOURCES: {
  kind: ThemeImportSourceKind;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
}[] = [
  { kind: "vscode", label: "VS Code", icon: Code2 },
  { kind: "shadcn", label: "shadcn / Tailwind", icon: Square },
  { kind: "file", label: "A .codemux-theme file", icon: FileJson },
];

export function ThemeImportSourcePicker({
  value,
  onChange,
}: {
  value: ThemeImportSourceKind;
  onChange: (kind: ThemeImportSourceKind) => void;
  /** Reserved for the Marketplace path, which hands parsed text straight in. */
  onApply?: (text: string) => void;
}) {
  return (
    <div className="flex flex-col gap-[7px]">
      <span className="text-[11.5px] font-semibold text-muted-foreground">
        Where is it from?
      </span>
      <div
        role="radiogroup"
        aria-label="Theme source"
        className="flex flex-col gap-[5px]"
      >
        {SOURCES.map(({ kind, label, icon: Icon }) => {
          const selected = value === kind;
          return (
            <button
              key={kind}
              type="button"
              role="radio"
              aria-checked={selected}
              onClick={() => onChange(kind)}
              className={cn(
                "flex h-9 items-center gap-2.5 rounded-[9px] border px-2.5 text-left transition-colors",
                selected
                  ? "border-accent-ember bg-accent-ember/10"
                  : "border-border/60 bg-muted/30 hover:bg-muted/50",
              )}
            >
              <Icon
                className={cn(
                  "size-3.5 flex-none",
                  selected ? "text-accent-ember" : "text-muted-foreground",
                )}
              />
              <span
                className={cn(
                  "flex-1 truncate text-[12px]",
                  selected ? "font-semibold text-foreground" : "text-muted-foreground",
                )}
              >
                {label}
              </span>
              {selected && <Check className="size-[11px] flex-none text-accent-ember" />}
            </button>
          );
        })}
      </div>
    </div>
  );
}
