import { useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { Globe, Image as ImageIcon, Trash2 } from "lucide-react";
import { resolveImageUrl } from "@/lib/project-image";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectName: string;
  initialValue: string | null;
  /**
   * Called with the raw user input (or null when cleared). The caller
   * decides how to persist it. The avatar resolves favicons on-render
   * so callers store exactly what the user typed.
   */
  onSave: (value: string | null) => void;
}

export function ProjectImageDialog({
  open,
  onOpenChange,
  projectName,
  initialValue,
  onSave,
}: Props) {
  const [value, setValue] = useState(initialValue ?? "");
  // Fresh token each time the dialog opens so the preview re-fetches the
  // favicon — lets the user see a site's *current* icon, not a cached one.
  const [previewBust, setPreviewBust] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setValue(initialValue ?? "");
      setPreviewBust(String(Date.now()));
    }
  }, [open, initialValue]);

  const resolved = resolveImageUrl(value, previewBust);
  const isWebsite = resolved.isFavicon;
  const isDirect = !isWebsite && resolved.url.length > 0;

  const handleSave = () => {
    onSave(value.trim() || null);
    onOpenChange(false);
  };

  const handleClear = () => {
    setValue("");
    onSave(null);
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="sm:max-w-[420px] bg-popover p-0 gap-0 overflow-hidden"
      >
        <DialogHeader className="px-5 pt-4 pb-2">
          <DialogTitle className="text-sm">Project image</DialogTitle>
          <DialogDescription className="sr-only">
            Set a custom image for {projectName}
          </DialogDescription>
        </DialogHeader>

        <div className="px-5 pb-4 space-y-4">
          {/* Live preview row */}
          <div className="flex items-center gap-3 rounded-lg border border-border/60 bg-muted/20 p-3">
            <ProjectAvatar
              name={projectName}
              imageUrl={value || null}
              cacheBust={previewBust}
              size="lg"
              shape="circle"
            />
            <div className="min-w-0 flex-1">
              <div className="text-sm text-foreground truncate">
                {projectName}
              </div>
              <div className="flex items-center gap-1 text-[11px] text-muted-foreground/70 mt-0.5">
                {isWebsite && (
                  <>
                    <Globe className="h-3 w-3 shrink-0" />
                    <span className="truncate">
                      Using favicon for {resolved.domain}
                    </span>
                  </>
                )}
                {isDirect && (
                  <>
                    <ImageIcon className="h-3 w-3 shrink-0" />
                    <span className="truncate">Direct image URL</span>
                  </>
                )}
                {!value.trim() && (
                  <span>Letter fallback</span>
                )}
              </div>
            </div>
          </div>

          {/* Input */}
          <div className="space-y-1.5">
            <label className="text-xs text-muted-foreground">
              Image URL or website
            </label>
            <Input
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder="codemux.com  or  https://…/logo.png"
              className="h-8 text-sm"
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter") handleSave();
              }}
            />
            <p className="text-[11px] text-muted-foreground/70 leading-snug">
              Paste a direct image URL, or any website to use its favicon.
            </p>
          </div>

          {/* Actions */}
          <div className="flex items-center justify-between pt-1">
            <Button
              variant="ghost"
              size="sm"
              className="h-8 px-2 text-xs text-muted-foreground hover:text-destructive"
              onClick={handleClear}
              disabled={!initialValue}
            >
              <Trash2 className="h-3.5 w-3.5 mr-1.5" />
              Remove image
            </Button>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => onOpenChange(false)}
              >
                Cancel
              </Button>
              <Button
                size="sm"
                onClick={handleSave}
                className="bg-foreground text-background hover:bg-foreground/90"
              >
                Save
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
