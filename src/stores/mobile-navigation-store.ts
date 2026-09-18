import { create } from "zustand";

// Survives lazy mounting, full-screen settings and breakpoint changes. A push
// link can arrive before the mobile shell has loaded its JavaScript chunk.
export const useMobileNavigationStore = create<{
  home: boolean;
  setHome: (home: boolean) => void;
}>((set) => ({ home: true, setHome: (home) => set({ home }) }));
