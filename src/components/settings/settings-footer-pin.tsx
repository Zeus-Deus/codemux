import { Pin, PinOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useFooterPinsStore } from "@/stores/footer-pins-store";
import type { Section } from "@/lib/settings-sections";

export function SettingsFooterPin({ section }: { section: Section }) {
  const id = `codemux.settings.${section}` as const;
  const pinned = useFooterPinsStore((state) =>
    state.pins.some((pin) => pin.id === id),
  );
  const togglePin = useFooterPinsStore((state) => state.togglePin);
  const Icon = pinned ? PinOff : Pin;
  return (
    <Button
      variant="ghost"
      size="sm"
      className="ml-auto text-muted-foreground"
      onClick={() => togglePin(id)}
    >
      <Icon className="size-3.5" />
      {pinned ? "Unpin from footer" : "Pin to footer"}
    </Button>
  );
}
