import { create } from "zustand";
export interface QuestionAttention {
  revision: number;
  workspaces: Record<string, number>;
}
export const useQuestionAttention = create<QuestionAttention>(() => ({
  revision: -1,
  workspaces: {},
}));
export function setQuestionAttention(snapshot: QuestionAttention) {
  if (snapshot.revision >= useQuestionAttention.getState().revision)
    useQuestionAttention.setState(snapshot);
}
