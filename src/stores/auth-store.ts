import { create } from "zustand";
import type { AuthUser } from "@/tauri/types";
import {
  checkAuth as checkAuthCmd,
  getSyncStatus as getSyncStatusCmd,
  startOauthFlow as startOauthFlowCmd,
  signinEmail as signinEmailCmd,
  signupEmail as signupEmailCmd,
  signOut as signOutCmd,
  type SyncStatus,
} from "@/tauri/commands";

export type AuthMethod = "email" | "github" | null;

interface AuthStore {
  user: AuthUser | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  isSigningIn: boolean;
  error: string | null;
  devBypass: boolean;

  // Skills sync (Stage 2). Backend keeps the encryption key bytes;
  // the frontend only sees a boolean + the auth method that picked
  // the user's session. The pair drives the Settings → Sync UI fork:
  //   - syncAvailable=true                        → "Sync ready"
  //   - syncAvailable=false, authMethod=github    → SetupSyncPasswordForm
  //   - syncAvailable=false, authMethod=email|nil → ProvidePasswordForm
  syncAvailable: boolean;
  authMethod: AuthMethod;

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
  setSyncStatus: (status: SyncStatus) => void;
  refreshSyncStatus: () => Promise<void>;
  clearError: () => void;
}

const DEV_USER: AuthUser = {
  id: "dev-local",
  email: "dev@localhost",
  name: "Dev Mode",
  image: null,
};

export const useAuthStore = create<AuthStore>((set, get) => ({
  user: null,
  isAuthenticated: false,
  isLoading: true,
  isSigningIn: false,
  error: null,
  devBypass: false,
  syncAvailable: false,
  authMethod: null,

  checkAuth: async () => {
    set({ isLoading: true, error: null });

    const maxRetries = 3;
    const retryDelay = 500;

    for (let attempt = 1; attempt <= maxRetries; attempt++) {
      try {
        const user = await checkAuthCmd();
        if (user) {
          set({ user, isAuthenticated: true, isLoading: false });
          // Pull the cold-start sync state. Backend may have loaded
          // the persisted sync-key.enc, in which case we want to
          // mark the session as sync-ready immediately rather than
          // waiting for the `sync-state-changed` event.
          try {
            const status = await getSyncStatusCmd();
            set({
              syncAvailable: status.syncAvailable,
              authMethod: status.authMethod,
            });
          } catch {
            // Tauri command unavailable in dev/test harness — leave
            // syncAvailable=false (already the default).
          }
        } else {
          set({
            user: null,
            isAuthenticated: false,
            isLoading: false,
            syncAvailable: false,
            authMethod: null,
          });
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
      syncAvailable: false,
      authMethod: null,
    });
  },

  setUser: (user) => {
    if (user) {
      set({ user, isAuthenticated: true, isSigningIn: false });
    } else {
      set({ user: null, isAuthenticated: false });
    }
  },

  setSyncStatus: (status) =>
    set({ syncAvailable: status.syncAvailable, authMethod: status.authMethod }),

  refreshSyncStatus: async () => {
    try {
      const status = await getSyncStatusCmd();
      set({
        syncAvailable: status.syncAvailable,
        authMethod: status.authMethod,
      });
    } catch {
      // Tauri unavailable — leave whatever we had before.
      void get();
    }
  },

  clearError: () => set({ error: null }),
}));
