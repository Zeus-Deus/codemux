import { writeToPty } from "@/tauri/commands";

// Separate IPC calls can reach the backend out of order (observed on Windows
// under load: typed "CORE_4" arrived as "E__4COR"). Each session therefore
// keeps at most one write in flight; input produced meanwhile is coalesced
// into the next write, so bytes reach the PTY in the order they were typed.
const sessions = new Map<string, { pending: string }>();

export function writePtyInput(sessionId: string, data: string): void {
  if (!data) return;
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
      try {
        await writeToPty(sessionId, batch);
      } catch (err) {
        console.error(`Failed to write to PTY for ${sessionId}:`, err);
      }
    }
    sessions.delete(sessionId);
  })();
}
