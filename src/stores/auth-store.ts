import { create } from "zustand";
import type { AuthUser } from "@/tauri/types";
import {
  checkAuth as checkAuthCmd,
  startOauthFlow as startOauthFlowCmd,
  signinEmail as signinEmailCmd,
  signupEmail as signupEmailCmd,
  signOut as signOutCmd,
} from "@/tauri/commands";

interface AuthStore {
  user: AuthUser | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  isSigningIn: boolean;
  error: string | null;
  devBypass: boolean;

  checkAuth: () => Promise<void>;
  startOAuthFlow: () => Promise<void>;
  signInEmail: (email: string, password: string) => Promise<void>;
  signUpEmail: (
    email: string,
    password: string,
    name: string,
  ) => Promise<void>;
  signOut: () => Promise<void>;
  setUser: (user: AuthUser | null) => void;
  clearError: () => void;
}

const DEV_USER: AuthUser = {
  id: "dev-local",
  email: "dev@localhost",
  name: "Dev Mode",
  image: null,
};

export const useAuthStore = create<AuthStore>((set) => ({
  user: null,
  isAuthenticated: false,
  isLoading: true,
  isSigningIn: false,
  error: null,
  devBypass: false,

  checkAuth: async () => {
    set({ isLoading: true, error: null });

    const maxRetries = 3;
    const retryDelay = 500;

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        const user = await checkAuthCmd();
        if (user) {
          set({ user, isAuthenticated: true, isLoading: false });
        } else {
          set({ user: null, isAuthenticated: false, isLoading: false });
        }
        return;
      } catch (err) {
        if (attempt < maxRetries) {
          await new Promise((resolve) => setTimeout(resolve, retryDelay));
        }
      }
    }

    set({
      user: DEV_USER,
      isAuthenticated: true,
      isLoading: false,
      devBypass: true,
    });
  },

  startOAuthFlow: async () => {
    set({ isSigningIn: true, error: null });
    try {
      await startOauthFlowCmd();
      // Don't set isSigningIn=false here — the OAuth callback
      // will trigger an auth-state-changed event which updates the store
    } catch (err) {
      set({
        isSigningIn: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  signInEmail: async (email, password) => {
    set({ isSigningIn: true, error: null });
    try {
      const resp = await signinEmailCmd(email, password);
      set({
        user: resp.user,
        isAuthenticated: true,
        isSigningIn: false,
      });
    } catch (err) {
      set({
        isSigningIn: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  signUpEmail: async (email, password, name) => {
    set({ isSigningIn: true, error: null });
    try {
      await signupEmailCmd(email, password, name);
      set({ isSigningIn: false });
    } catch (err) {
      set({
        isSigningIn: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  signOut: async () => {
    try {
      await signOutCmd();
    } catch {
      // Ignore errors — clear local state regardless
    }
    set({
      user: null,
      isAuthenticated: false,
      isSigningIn: false,
      devBypass: false,
    });
  },

  setUser: (user) => {
    if (user) {
      set({ user, isAuthenticated: true, isSigningIn: false });
    } else {
      set({ user: null, isAuthenticated: false });
    }
  },

  clearError: () => set({ error: null }),
}));
