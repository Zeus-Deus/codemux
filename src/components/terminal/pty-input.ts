import { isRemoteClient } from "@/components/remote/is-remote-client";
import { writeToPty } from "@/tauri/commands";

// Separate IPC calls can reach the backend out of order (observed on Windows
// under load: typed "CORE_4" arrived as "E__4COR"). Each session therefore
// keeps at most one write in flight; input produced meanwhile is coalesced
// into the next write, so bytes reach the PTY in the order they were typed.
// A remote client does not wait: its connection delivers writes in order and
// the server applies them in arrival order, so waiting for each reply would
// only delay every key typed meanwhile by a network round trip.
const sessions = new Map<string, { pending: string }>();

function send(sessionId: string, data: string) {
  return writeToPty(sessionId, data).catch((err) => {
    console.error(`Failed to write to PTY for ${sessionId}:`, err);
  });
}

export function writePtyInput(sessionId: string, data: string): void {
  if (!data) return;
  if (isRemoteClient()) {
    void send(sessionId, data);
    return;
  }
  const active = sessions.get(sessionId);
  if (active) {
    active.pending += data;
    return;
  }
  const session = { pending: data };
  sessions.set(sessionId, session);
  void (async () => {
    while (session.pending) {
      const batch = session.pending;
      session.pending = "";
      await send(sessionId, batch);
    }
    sessions.delete(sessionId);
  })();
}
