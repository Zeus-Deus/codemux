import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import {
  setQuestionAttention,
  type QuestionAttention,
} from "@/stores/async-question-attention-store";

/** Low-frequency durable attention, independent of running/permission status. */
export function useAsyncQuestionAttention() {
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<QuestionAttention>("agent-chat-question-attention", (e) => {
      if (!disposed) setQuestionAttention(e.payload);
    })
      .then(async (stop) => {
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        const snapshot = await invoke<QuestionAttention>(
          "agent_chat_question_attention",
        );
        if (!disposed && snapshot) setQuestionAttention(snapshot);
      })
      .catch((error) =>
        console.warn(
          "[agent-chat] Could not subscribe to question attention",
          error,
        ),
      );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
