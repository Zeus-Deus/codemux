import { useState } from "react";
import { ArrowUp, ArrowDown, Plus, X, RotateCcw, Check } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  FOOTER_ACTIONS,
  FOOTER_ICONS,
  getFooterAction,
  isFooterActionAvailable,
  type FooterIconId,
} from "@/lib/footer-actions";
import { useFooterPinsStore, type FooterPin } from "@/stores/footer-pins-store";
import { useFooterAvailability } from "./footer-availability";

function IconPicker({ pin }: { pin: FooterPin }) {
  const [open, setOpen] = useState(false);
  const setIcon = useFooterPinsStore((s) => s.setIcon);
  const action = getFooterAction(pin.id)!;
  const Icon = pin.iconId ? FOOTER_ICONS[pin.iconId] : action.icon;
  const choose = (id?: FooterIconId) => {
    setIcon(pin.id, id);
    setOpen(false);
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label={`Choose icon for ${action.label}`}
          title="Choose icon"
        >
          <Icon className="size-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-64 p-3" align="start">
        <p className="mb-2 text-xs font-medium">Choose a bundled icon</p>
        <Button
          variant="ghost"
          size="sm"
          className="mb-2 w-full justify-start"
          onClick={() => choose()}
        >
          <action.icon className="size-4" /> Default icon{" "}
          {!pin.iconId && <Check className="ml-auto size-3" />}
        </Button>
        <div className="grid grid-cols-4 gap-1">
          {Object.entries(FOOTER_ICONS).map(([id, Glyph]) => (
            <Button
              key={id}
              variant={pin.iconId === id ? "secondary" : "ghost"}
              size="icon-sm"
              aria-label={`${id} icon`}
              aria-pressed={pin.iconId === id}
              title={id}
              onClick={() => choose(id as FooterIconId)}
            >
              <Glyph className="size-4" />
            </Button>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

export function CustomizeFooterDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { pins, togglePin, movePin, reset } = useFooterPinsStore();
  const { agentChatEnabled, hasDevices } = useFooterAvailability();
  const [search, setSearch] = useState("");
  const available = FOOTER_ACTIONS.filter(
    (action) =>
      isFooterActionAvailable(action, agentChatEnabled, hasDevices) &&
      !pins.some((pin) => pin.id === action.id) &&
      action.label.toLowerCase().includes(search.toLowerCase()),
  );
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-lg">
        <DialogHeader className="border-b px-6 py-5">
          <DialogTitle>Customize footer</DialogTitle>
          <DialogDescription>
            Keep your frequent destinations close. Changes save on this device.
          </DialogDescription>
        </DialogHeader>
        <div className="thin-scrollbar min-h-0 overflow-y-auto px-6 py-4">
          <h3 className="mb-3 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
            Pinned destinations · {pins.length}
          </h3>
          {pins.length === 0 && (
            <p className="py-3 text-sm text-muted-foreground">
              No pins yet. Your app menu stays in the footer.
            </p>
          )}
          <ol className="space-y-1">
            {pins.map((pin, index) => {
              const action = getFooterAction(pin.id)!;
              const available = isFooterActionAvailable(
                action,
                agentChatEnabled,
                hasDevices,
              );
              return (
                <li
                  key={pin.id}
                  className="flex items-center gap-2 rounded-lg border border-border/60 p-2"
                >
                  <IconPicker pin={pin} />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">
                      {action.label}
                    </p>
                    {!available && (
                      <p className="text-xs text-muted-foreground">
                        Hidden until available
                      </p>
                    )}
                  </div>
                  <div className="flex shrink-0">
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      aria-label={`Move ${action.label} up`}
                      disabled={index === 0}
                      onClick={() => movePin(pin.id, -1)}
                    >
                      <ArrowUp className="size-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      aria-label={`Move ${action.label} down`}
                      disabled={index === pins.length - 1}
                      onClick={() => movePin(pin.id, 1)}
                    >
                      <ArrowDown className="size-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      aria-label={`Remove ${action.label}`}
                      onClick={() => togglePin(pin.id)}
                    >
                      <X className="size-3.5" />
                    </Button>
                  </div>
                </li>
              );
            })}
          </ol>
          <h3 className="mb-3 mt-6 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
            Add a destination
          </h3>
          <Input
            aria-label="Find footer destinations"
            placeholder="Find a setting or destination…"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <div className="mt-2">
            {available.map((action) => (
              <button
                key={action.id}
                type="button"
                aria-label={`Pin ${action.label}`}
                className="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left text-sm hover:bg-muted focus-visible:outline-ring"
                onClick={() => togglePin(action.id)}
              >
                <action.icon className="size-4 text-muted-foreground" />
                <span className="flex-1">{action.label}</span>
                <Plus className="size-3.5 text-muted-foreground" />
              </button>
            ))}
            {available.length === 0 && (
              <p className="py-3 text-sm text-muted-foreground">
                No matching destinations to add.
              </p>
            )}
          </div>
        </div>
        <div className="flex items-center justify-between border-t px-6 py-4">
          <Button variant="ghost" size="sm" onClick={reset}>
            <RotateCcw className="size-3.5" />
            Restore defaults
          </Button>
          <Button size="sm" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
