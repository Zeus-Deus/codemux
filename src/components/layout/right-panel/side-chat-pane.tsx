/**
 * Side chat pane — the answer to a `/btw` question.
 *
 * The question is asked of a read-only fork of the focused chat's provider
 * session, so it can use everything the main thread has said without adding
 * anything back. Each question renders as the transcript's own user bubble
 * and each answer as its assistant message. The reply box wears the main
 * composer's surface and sits at the main composer's height, with one quiet
 * line under it naming the model that answers (the main chat's, at the main
 * chat's effort). The reply box continues the same fork; closing the pane
 * disposes it.
 */
import { ArrowUp, CircleAlert } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";

import { AssistantMessage } from "@/components/chat/AssistantMessage";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { UserMessage } from "@/components/chat/UserMessage";
import {
  COMPOSER_CARD,
  COMPOSER_ROUND_CONTROL,
  COMPOSER_SEND,
} from "@/components/chat/composer-surface";
import { Empty, EmptyDescription, EmptyTitle } from "@/components/ui/empty";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
} from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import { fallbackModelLabel } from "@/lib/agent-chat/capability-defaults";
import { sessionProviderLabel } from "@/lib/agent-chat/session-mentions";
import {
  lastSideChatTurn,
  sendSideChat,
  type SideChatState,
} from "@/lib/agent-chat/side-chat";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { cn } from "@/lib/utils";
import type { AgentChatProviderKind } from "@/tauri/types";

/** Display name of the model a side chat answers with. The provider logo
 *  sits beside it, so a leading provider name ("Claude Opus 4.8") is
 *  dropped. With no catalogue entry the id is humanised; with no id at all
 *  it is the provider's default model. */
export function sideChatModelName(
  provider: AgentChatProviderKind,
  modelId: string | null,
  catalogLabel: string | null,
): string {
  const label = catalogLabel ?? (modelId ? fallbackModelLabel(modelId) : "Default model");
  const prefix = `${sessionProviderLabel(provider)} `;
  return label.toLowerCase().startsWith(prefix.toLowerCase()) && label.length > prefix.length
    ? label.slice(prefix.length)
    : label;
}

/** Display name of a reasoning effort, from the provider's own label map. */
export function sideChatEffortName(
  effort: string | null,
  labels: Record<string, string>,
): string | null {
  if (!effort) return null;
  return labels[effort] ?? effort.charAt(0).toUpperCase() + effort.slice(1);
}

/** "<logo> Opus 4.8 · High": which model answers, at which effort. */
function SideChatModelRow({ chat }: { chat: SideChatState }) {
  const caps = useProviderCapabilities((s) => selectCapabilities(s, chat.provider));
  const model = chat.model ? selectModel(caps, chat.model) : (caps?.models[0] ?? null);
  const modelName = sideChatModelName(chat.provider, chat.model, model?.label ?? null);
  const effortName = sideChatEffortName(chat.effort, caps?.effort_label_map ?? {});
  const description = `${sessionProviderLabel(chat.provider)} ${modelName}${
    effortName ? ` at ${effortName} effort` : ""
  }`;
  return (
    <div
      data-testid="side-chat-model"
      title={`Answered by ${description}`}
      // 23px: the gap between the reply box and the deck's status foot when
      // the box's top lines up with the main composer's (whose 35px scope
      // strip and 12px dock padding sit where this line and the foot are).
      className="flex h-[23px] shrink-0 items-center gap-1.5 px-4 text-label text-muted-foreground"
    >
      <ProviderLogo provider={chat.provider} className="size-3.5" />
      <span className="sr-only">Answered by </span>
      <span className="truncate font-medium text-foreground/85">{modelName}</span>
      {effortName && (
        <>
          <span aria-hidden>·</span>
          <span className="shrink-0">
            {effortName}
            <span className="sr-only"> effort</span>
          </span>
        </>
      )}
    </div>
  );
}

export function SideChatPane({
  chat,
  workspaceId,
  cwd,
}: {
  chat: SideChatState | null;
  workspaceId: string;
  cwd: string | null;
}) {
  const [reply, setReply] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const turnCount = chat?.turns.length ?? 0;
  const lastAnswer = lastSideChatTurn(chat)?.answer ?? "";

  // Follow the streaming answer only while the reader is already at the
  // bottom, so scrolling up to reread isn't yanked back.
  const pinnedRef = useRef(true);
  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  }, []);
  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [turnCount, lastAnswer]);

  if (!chat) {
    return (
      <div className="flex h-full items-center justify-center">
        <Empty>
          <EmptyTitle>No side chat open</EmptyTitle>
          <EmptyDescription>
            Type <span className="font-mono text-foreground">/btw</span> and a question in the
            chat to ask something on the side. The answer uses the chat's context and adds
            nothing to it.
          </EmptyDescription>
        </Empty>
      </div>
    );
  }

  const busy = lastSideChatTurn(chat)?.status === "streaming";
  const canReply = !chat.ended && !busy && reply.trim().length > 0;
  const submit = () => {
    if (!canReply) return;
    const text = reply.trim();
    setReply("");
    pinnedRef.current = true;
    void sendSideChat(chat.sideId, text);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit();
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-background" data-testid="side-chat-pane">
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-y-auto px-3 pb-3.5 pt-3"
      >
        <div className="flex flex-col gap-4">
          {chat.turns.map((turn, index) => (
            <div key={index} className="flex flex-col gap-2.5">
              {/* The transcript's own components, so a side question reads
                  exactly like a main one. Neither gets a handler, so the
                  bubble shows only its Copy action. */}
              <UserMessage
                item={{
                  kind: "user_message",
                  id: `${chat.sideId}-q${index}`,
                  seq: index,
                  text: turn.question,
                }}
              />
              {turn.answer ? (
                <AssistantMessage
                  item={{
                    kind: "assistant_message",
                    id: `${chat.sideId}-a${index}`,
                    seq: index,
                    turn_id: null,
                    text: turn.answer,
                    streaming: turn.status === "streaming",
                  }}
                  workspaceId={workspaceId}
                  cwd={cwd}
                />
              ) : turn.status === "streaming" ? (
                <div className="flex items-center gap-2 text-label text-muted-foreground">
                  <Spinner className="size-3" />
                  <span className="shimmer">Thinking…</span>
                </div>
              ) : null}
              {turn.status === "error" && (
                <p role="alert" className="flex items-start gap-1.5 text-label text-danger">
                  <CircleAlert className="mt-px size-3.5 shrink-0" aria-hidden />
                  <span>{turn.error ?? "The side chat failed."}</span>
                </p>
              )}
            </div>
          ))}
        </div>
      </div>
      <div className="shrink-0 px-3 pt-2">
        {/* The main composer's own surface and 34px send circle, so the two
            boxes read as one control family side by side. */}
        <InputGroup
          className={cn(
            // The primitive's field chrome gives way to the composer card;
            // its focus ring too, since the card shows focus by surface.
            "h-auto min-h-11 items-end border-border/80",
            // Its dark-mode field tint outranks the card's surface; restate it.
            "dark:bg-[color-mix(in_oklab,var(--muted)_40%,var(--background))] dark:focus-within:bg-[color-mix(in_oklab,var(--muted)_60%,var(--background))]",
            // The primitive dims the whole group when ANY child is disabled,
            // which includes the send button on an empty reply. Dim only
            // when the box itself is closed (the side chat has ended).
            "has-disabled:bg-[color-mix(in_oklab,var(--muted)_40%,var(--background))] has-disabled:opacity-100",
            "dark:has-disabled:bg-[color-mix(in_oklab,var(--muted)_40%,var(--background))] has-[textarea:disabled]:opacity-60",
            "has-[[data-slot=input-group-control]:focus-visible]:border-border/80 has-[[data-slot=input-group-control]:focus-visible]:ring-0",
            COMPOSER_CARD,
          )}
        >
          <InputGroupTextarea
            value={reply}
            rows={1}
            disabled={chat.ended}
            onChange={(event) => setReply(event.target.value)}
            onKeyDown={onKeyDown}
            aria-label="Reply in this side chat"
            placeholder={chat.ended ? "This side chat has ended" : "Reply in this side chat…"}
            // The main composer's text layout: 14px conversation type and
            // 10px/12px padding. A whole-pixel 22px line keeps the box at
            // exactly 44px (relaxed leading would make it 44.75px and drop
            // the send circle off-centre).
            className="conversation-text max-h-32 min-h-0 px-3 py-2.5 leading-[22px] placeholder:text-muted-foreground/60"
          />
          {/* 1px border + 4px = the main composer's 5px inset on every side.
              The primitive pulls an end button 0.3rem outward; undo that. */}
          <InputGroupAddon align="inline-end" className="self-end py-1 pr-1 has-[>button]:mr-0">
            <InputGroupButton
              aria-label="Send reply"
              title="Send"
              disabled={!canReply}
              onClick={submit}
              className={cn(COMPOSER_ROUND_CONTROL, COMPOSER_SEND, "p-0 has-[>svg]:p-0")}
            >
              <ArrowUp className="size-4" aria-hidden />
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        <SideChatModelRow chat={chat} />
      </div>
    </div>
  );
}
