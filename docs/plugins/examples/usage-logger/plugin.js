// Milestone 5 check: logs every usage_recorded the chat tap delivers, for
// every thread (including ones with no pane open), with its cache TTL tier,
// and what that thread's session is doing right then (host.chat.getStatus).
// Plain ESM, no JSX.
import { host, PALETTE_AREA } from "@codemux/sdk";

export default {
  id: "usage-logger",
  name: "Usage logger",
  description: "Logs usage_recorded events from every chat thread",
  version: "0.1.0",
  register(ctx) {
    const seen = [];
    // Exposed for inspection only.
    window.__usageLogger = seen;
    ctx.onDispose(
      host.chat.onEvent(({ threadId, event, cacheWriteTtl, subagent }) => {
        if (event.type !== "usage_recorded") return;
        const row = {
          threadId,
          provider: event.provider,
          cacheWriteTtl: cacheWriteTtl ?? null,
          subagent: !!subagent,
          cacheWriteTokens: event.cache_write_tokens,
        };
        seen.push(row);
        // getStatus is async: use ctx.host for anything done after it.
        void ctx.host.chat.getStatus(threadId).then((status) => {
          // null: the thread has no live session (closed, or never started).
          row.status = status && { busy: status.busy, queued: status.queued, status: status.status };
          console.info("[usage-logger]", JSON.stringify(row));
        });
      }),
    );
    ctx.register({
      id: "show",
      area: PALETTE_AREA,
      data: {
        label: "Usage logger: show last event",
        run: () => {
          const last = seen[seen.length - 1];
          host.notify({
            message: last
              ? `${last.provider} usage, cache TTL ${last.cacheWriteTtl ?? "none"}, ` +
                (last.status === undefined
                  ? "status pending"
                  : last.status
                    ? `session ${last.status.status}${last.status.busy ? " (busy)" : ""}`
                    : "no live session")
              : "No usage events yet",
          });
        },
      },
    });
  },
};
