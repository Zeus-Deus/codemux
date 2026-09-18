import { useState } from "react";
import { Check, ChevronDown, CornerDownRight, ListEnd, Square } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Command, CommandGroup, CommandItem, CommandList } from "@/components/ui/command";
import {
  MESSAGE_DELIVERY_OPTIONS,
  STEERING_UNAVAILABLE,
  type MessageDelivery,
} from "@/lib/agent-chat/message-delivery";
import { focusCmdkOnOpen } from "./pickers/focus-cmdk-root";

const deliveryIcons = { queue: ListEnd, steer: CornerDownRight, interrupt: Square };

export function MessageDeliveryPicker({
  value,
  supportsSteering,
  disabled,
  onChange,
}: {
  value: MessageDelivery;
  supportsSteering: boolean;
  disabled?: boolean;
  onChange: (value: MessageDelivery) => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = MESSAGE_DELIVERY_OPTIONS.find((option) => option.value === value)!;
  const Icon = deliveryIcons[value];
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          aria-label={`Message delivery: ${selected.label}`}
          title={selected.description}
          className="inline-flex h-[34px] shrink-0 items-center gap-1.5 rounded-full px-2 text-label text-muted-foreground hover:bg-muted/60 hover:text-foreground disabled:opacity-40"
        >
          <Icon className="size-3.5" aria-hidden />
          <span>{value === "interrupt" ? "Interrupt" : selected.label}</span>
          <ChevronDown className="size-3" aria-hidden />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        side="top"
        className="w-[320px] max-w-[calc(100vw-24px)] p-0"
        onOpenAutoFocus={focusCmdkOnOpen}
      >
        <Command>
          <CommandList>
            <CommandGroup heading="Send while the agent is working">
              {MESSAGE_DELIVERY_OPTIONS.map((option) => {
                const unavailable = option.value === "steer" && !supportsSteering;
                const OptionIcon = deliveryIcons[option.value];
                return (
                  <CommandItem
                    key={option.value}
                    value={option.value}
                    disabled={unavailable}
                    onSelect={() => {
                      onChange(option.value);
                      setOpen(false);
                    }}
                    className="items-start gap-2 py-2"
                  >
                    <OptionIcon className="mt-0.5 size-3.5 shrink-0" aria-hidden />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center justify-between text-label">
                        <span>{option.label}</span>
                        <span className="text-muted-foreground">/{option.value}</span>
                      </span>
                      <span className="mt-1 block text-label leading-relaxed text-muted-foreground">
                        {unavailable ? STEERING_UNAVAILABLE : option.description}
                      </span>
                    </span>
                    {value === option.value && (
                      <Check className="mt-0.5 size-3 shrink-0" aria-hidden />
                    )}
                  </CommandItem>
                );
              })}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
