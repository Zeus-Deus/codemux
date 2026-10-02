import { useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { filterCommandMenuItems, groupSlashItems, type SlashCommandItem } from "@/lib/agent-chat/slash-commands";
import { CommandSourceIcon } from "./CommandSourceIcon";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providerLabel: string;
  items: SlashCommandItem[];
  onSelect: (item: SlashCommandItem) => void;
  loading: boolean;
  error: string | null;
}

/** Uses the composer's executable catalogue, including provider additions. */
export function ProviderCommandsDialog({ open, onOpenChange, ...props }: Props) {
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-2xl">
      <ProviderCommandsBody onOpenChange={onOpenChange} {...props} />
    </DialogContent>
  </Dialog>;
}

// Mounted only while the dialog is open: the composer re-renders on every
// slash-menu highlight move, and building this catalogue each time made
// held arrow keys fall behind. Unmounting also resets the search.
function ProviderCommandsBody({ onOpenChange, providerLabel, items, onSelect, loading, error }: Omit<Props, "open">) {
  const [query, setQuery] = useState("");
  const groups = groupSlashItems(filterCommandMenuItems(items.filter((item) => item.id !== "composer:help"), query));
  return <>
      <DialogHeader className="pr-8">
        <DialogTitle>{providerLabel} commands and skills</DialogTitle>
        <DialogDescription>Browse commands, installed skills, and chat controls for this directory. Commands are prepared in the composer; controls open immediately.</DialogDescription>
      </DialogHeader>
      <input type="search" aria-label="Search commands and skills" placeholder="Search commands and skills…" value={query} onChange={(event) => setQuery(event.target.value)} className="w-full rounded-md border bg-transparent px-3 py-2 outline-none focus:ring-1 focus:ring-ring" />
      {loading && <p role="status" className="text-muted-foreground">Discovering commands and skills…</p>}
      {error && <p role="alert" className="break-words text-destructive">{error}</p>}
      <div className="min-h-0 space-y-4 overflow-y-auto">
        {groups.map((group) => <section key={group.group}>
          <h3 className="mb-1 text-label font-medium text-muted-foreground">{group.group}</h3>
          {group.items.map((item) => <button key={item.id} type="button" disabled={item.disabled} onClick={() => { onOpenChange(false); onSelect(item); }} className="flex w-full items-start gap-3 rounded-md px-3 py-2 text-left hover:bg-accent disabled:opacity-50">
            {item.identity && <CommandSourceIcon identity={item.identity} />}
            <span className="min-w-0 flex-1"><span className="block font-medium">{item.label}</span>{item.identity && <span className="mt-1 block text-label text-muted-foreground">{item.identity.label}</span>}<span className="mt-1 block text-label text-muted-foreground">{item.description}{item.argumentHint ? ` · ${item.argumentHint}` : ""}</span></span>
            <code className="max-w-[40%] shrink-0 truncate text-label text-muted-foreground" title={item.command}>{item.command}</code>
          </button>)}
        </section>)}
        {groups.length === 0 && <p className="text-muted-foreground">No commands or skills match.</p>}
      </div>
  </>;
}
