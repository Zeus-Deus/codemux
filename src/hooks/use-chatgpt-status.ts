import { useEffect } from "react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { onChatGptStatusChanged } from "@/tauri/events";

export function useChatGptStatus() {
  const phase = useChatGptStore((state) => state.status?.phase);
  useEffect(() => {
    if (isRemoteClient()) { void useChatGptStore.getState().bootstrap(); return; }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void useChatGptStore.getState().bootstrap();
    void onChatGptStatusChanged(() => { void useChatGptStore.getState().refresh(); })
      .then((dispose) => { if (disposed) dispose(); else unlisten = dispose; })
      .catch(() => {
        if (!disposed) useChatGptStore.setState({ subscriptionError:
          "Connection updates are unavailable. Use Refresh to check your ChatGPT connection." });
      });
    return () => { disposed = true; unlisten?.(); };
  }, []);
  useEffect(() => {
    if (phase !== "pending" || isRemoteClient()) return;
    // Native auth owns the deadline. Polling also recovers a missed event.
    const timer = window.setInterval(() => { void useChatGptStore.getState().refresh(); }, 1000);
    return () => window.clearInterval(timer);
  }, [phase]);
}
