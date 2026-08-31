import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock the Tauri commands before importing the store
vi.mock("@/tauri/commands", () => ({
  bootstrapSession: vi.fn(),
  refreshSession: vi.fn(),
  startOauthFlow: vi.fn(),
  signinEmail: vi.fn(),
  signupEmail: vi.fn(),
  signOut: vi.fn(),
  getSyncStatus: vi.fn(),
  // Pulled in transitively through the provider capability/health stores.
  listChatProviderCapabilities: vi.fn(),
  agentChatProviderHealth: vi.fn(),
}));

import { useAuthStore } from "./auth-store";
import {
  bootstrapSession,
  refreshSession,
  signinEmail,
  signupEmail,
  signOut,
  getSyncStatus,
} from "@/tauri/commands";
import { DEFAULT_SETTINGS } from "./synced-settings-store";
import { useProviderRuntimeIntent } from "./provider-runtime-intent-store";
import {
  PROVIDER_CAPABILITIES_STORAGE_KEY,
  useProviderCapabilities,
} from "./provider-capabilities-store";
import type { ProviderChatCapabilities } from "@/tauri/types";

const mockBootstrapSession = vi.mocked(bootstrapSession);
const mockRefreshSession = vi.mocked(refreshSession);
const mockSigninEmail = vi.mocked(signinEmail);
const mockSignupEmail = vi.mocked(signupEmail);
const mockSignOut = vi.mocked(signOut);
const mockGetSyncStatus = vi.mocked(getSyncStatus);

const mockUser = { id: "u1", email: "a@b.com", name: "Test", image: null };

function minimalCaps(): ProviderChatCapabilities {
  return {
    models: [],
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

function session(
  status:
    | "local"
    | "verified"
    | "offline"
    | "pending-verification"
    | "signed-out",
  authenticated = status !== "signed-out",
) {
  return {
    authenticated,
    user: authenticated ? mockUser : null,
    settings: DEFAULT_SETTINGS,
    authMethod: authenticated ? ("email" as const) : null,
    status,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  useProviderRuntimeIntent.getState().reset();
  mockBootstrapSession.mockResolvedValue(session("local"));
  mockRefreshSession.mockResolvedValue(session("verified"));
  // Reset store to initial state
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    isLoading: true,
    isSigningIn: false,
    error: null,
    devBypass: false,
    sessionStatus: "signed-out",
    syncAvailable: false,
    authMethod: null,
  });
});

describe("auth store", () => {
  it("starts in loading state", () => {
    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(true);
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  it("bootstrapSession paints a valid cached session without remote verification", async () => {
    mockBootstrapSession.mockResolvedValue(session("local"));

    await useAuthStore.getState().bootstrapSession();

    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(false);
    expect(state.isAuthenticated).toBe(true);
    expect(state.user).toEqual(mockUser);
    expect(state.sessionStatus).toBe("local");
    expect(mockRefreshSession).not.toHaveBeenCalled();
  });

  it("bootstrapSession renders signed out when no local token exists", async () => {
    mockBootstrapSession.mockResolvedValue(session("signed-out", false));

    await useAuthStore.getState().bootstrapSession();

    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(false);
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  it("does not invent a dev account when local bootstrap fails", async () => {
    mockBootstrapSession.mockRejectedValue(new Error("Local database failed"));

    await useAuthStore.getState().bootstrapSession();

    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(false);
    expect(state.isAuthenticated).toBe(false);
    expect(state.devBypass).toBe(false);
    expect(state.user).toBeNull();
  });

  it("signInEmail sets user on success", async () => {
    mockSigninEmail.mockResolvedValue({
      token: "tok",
      expires_at: "2099-01-01",
      user: { id: "u1", email: "a@b.com", name: "Test", image: null },
    });

    await useAuthStore.getState().signInEmail("a@b.com", "pass");

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(true);
    expect(state.isSigningIn).toBe(false);
    expect(state.user?.email).toBe("a@b.com");
    expect(mockRefreshSession).toHaveBeenCalledTimes(1);
  });

  it("signInEmail sets error on failure", async () => {
    mockSigninEmail.mockRejectedValue(new Error("Invalid credentials"));

    await useAuthStore.getState().signInEmail("a@b.com", "wrong");

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(false);
    expect(state.isSigningIn).toBe(false);
    expect(state.error).toBe("Invalid credentials");
  });

  it("signUpEmail succeeds without authenticating (email verification required)", async () => {
    mockSignupEmail.mockResolvedValue(undefined);

    await useAuthStore.getState().signUpEmail("new@b.com", "pass", "New");

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(false);
    expect(state.isSigningIn).toBe(false);
    expect(state.user).toBeNull();
    expect(state.error).toBeNull();
  });

  it("signInEmail shows error when email not verified", async () => {
    mockSigninEmail.mockRejectedValue(new Error("Email not verified"));

    await useAuthStore.getState().signInEmail("a@b.com", "pass");

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(false);
    expect(state.isSigningIn).toBe(false);
    expect(state.error).toBe("Email not verified");
  });

  it("signOut clears user and calls command", async () => {
    mockSignOut.mockResolvedValue(undefined);

    // Set up authenticated state
    useAuthStore.setState({
      user: { id: "u1", email: "a@b.com", name: "T", image: null },
      isAuthenticated: true,
      isLoading: false,
    });

    await useAuthStore.getState().signOut();

    expect(mockSignOut).toHaveBeenCalled();
    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  it("does not carry provider runtime intent across sign-out and sign-in", async () => {
    mockSignOut.mockResolvedValue(undefined);
    mockSigninEmail.mockResolvedValue({
      token: "tok",
      expires_at: "2099-01-01",
      user: mockUser,
    });
    useAuthStore.setState({
      user: mockUser,
      isAuthenticated: true,
      isLoading: false,
    });
    useProviderRuntimeIntent.getState().observe("claude");

    await useAuthStore.getState().signOut();
    expect(useProviderRuntimeIntent.getState().providers).toEqual({});

    await useAuthStore.getState().signInEmail("a@b.com", "pass");
    expect(useProviderRuntimeIntent.getState().providers).toEqual({});
  });

  it("clearError clears the error message", () => {
    useAuthStore.setState({ error: "some error" });
    useAuthStore.getState().clearError();
    expect(useAuthStore.getState().error).toBeNull();
  });

  it("setUser updates auth state", () => {
    const user = { id: "u1", email: "a@b.com", name: "T", image: null };
    useAuthStore.getState().setUser(user);

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(true);
    expect(state.user).toEqual(user);
  });

  it("setUser(null) clears auth state", () => {
    useAuthStore.setState({
      user: { id: "u1", email: "a@b.com", name: "T", image: null },
      isAuthenticated: true,
    });

    useAuthStore.getState().setUser(null);

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  // ── Skills sync (Stage 2) ──

  it("syncAvailable defaults to false", () => {
    expect(useAuthStore.getState().syncAvailable).toBe(false);
    expect(useAuthStore.getState().authMethod).toBeNull();
  });

  it("setSyncStatus updates sync state from a backend payload", () => {
    useAuthStore.getState().setSyncStatus({
      syncAvailable: true,
      authMethod: "github",
    });
    const state = useAuthStore.getState();
    expect(state.syncAvailable).toBe(true);
    expect(state.authMethod).toBe("github");
  });

  it("checkAuth applies the combined remote auth/settings result", async () => {
    mockRefreshSession.mockResolvedValue(session("verified"));

    await useAuthStore.getState().checkAuth();

    expect(mockRefreshSession).toHaveBeenCalledTimes(1);
    const state = useAuthStore.getState();
    expect(state.syncAvailable).toBe(true);
    expect(state.authMethod).toBe("email");
  });

  it("remote invocation failure retains an already-painted local session", async () => {
    useAuthStore.setState({
      user: mockUser,
      isAuthenticated: true,
      isLoading: false,
      sessionStatus: "local",
    });
    mockRefreshSession.mockRejectedValue(new Error("offline"));

    await useAuthStore.getState().checkAuth();

    const state = useAuthStore.getState();
    expect(state.isAuthenticated).toBe(true);
    expect(state.sessionStatus).toBe("offline");
  });

  it("signOut clears sync state alongside auth state", async () => {
    mockSignOut.mockResolvedValue(undefined);
    useAuthStore.setState({
      user: { id: "u1", email: "a@b.com", name: "T", image: null },
      isAuthenticated: true,
      syncAvailable: true,
      authMethod: "github",
    });

    await useAuthStore.getState().signOut();

    const state = useAuthStore.getState();
    expect(state.syncAvailable).toBe(false);
    expect(state.authMethod).toBeNull();
  });

  it("checkAuth → definitive signed-out result clears sync state", async () => {
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
    mockRefreshSession.mockResolvedValue(session("signed-out", false));

    await useAuthStore.getState().checkAuth();

    const state = useAuthStore.getState();
    expect(state.syncAvailable).toBe(false);
    expect(state.authMethod).toBeNull();
  });

  it("refreshSyncStatus pulls from getSyncStatus", async () => {
    mockGetSyncStatus.mockResolvedValue({
      syncAvailable: true,
      authMethod: "email",
    });

    await useAuthStore.getState().refreshSyncStatus();

    const state = useAuthStore.getState();
    expect(state.syncAvailable).toBe(true);
    expect(state.authMethod).toBe("email");
  });

  it("concurrent refreshSession calls share one in-flight verification", async () => {
    let resolveRefresh!: (value: ReturnType<typeof session>) => void;
    mockRefreshSession.mockReturnValue(
      new Promise((resolve) => {
        resolveRefresh = resolve;
      }),
    );

    const first = useAuthStore.getState().refreshSession();
    const second = useAuthStore.getState().refreshSession();

    expect(mockRefreshSession).toHaveBeenCalledTimes(1);
    resolveRefresh(session("verified"));
    await Promise.all([first, second]);
    expect(useAuthStore.getState().sessionStatus).toBe("verified");

    // Once settled, a later call issues a fresh verification.
    mockRefreshSession.mockResolvedValue(session("verified"));
    await useAuthStore.getState().refreshSession();
    expect(mockRefreshSession).toHaveBeenCalledTimes(2);
  });

  it("transient verification failure keeps a pending session pending", async () => {
    // A valid cached token with no verified user paints the login frame in
    // `pending-verification`. An invoke-level failure is not an authoritative
    // rejection, so the session must stay pending for the post-paint retry
    // path instead of downgrading to signed-out.
    useAuthStore.setState({
      isLoading: false,
      sessionStatus: "pending-verification",
    });
    mockRefreshSession.mockRejectedValue(new Error("ipc channel closed"));

    await useAuthStore.getState().refreshSession();

    const state = useAuthStore.getState();
    expect(state.sessionStatus).toBe("pending-verification");
    expect(state.isAuthenticated).toBe(false);
    expect(state.error).toBe("ipc channel closed");
  });

  it("resolved unreachable-verify without a cached identity keeps the session pending", async () => {
    // The backend RESOLVES a timed-out/5xx verify over a valid token with no
    // cached user as still `pending-verification` (not `offline`), so the
    // post-paint retry schedule keeps re-verifying instead of stranding a
    // valid token on the login screen.
    useAuthStore.setState({
      isLoading: false,
      sessionStatus: "pending-verification",
    });
    mockRefreshSession.mockResolvedValue(
      session("pending-verification", false),
    );

    await useAuthStore.getState().refreshSession();

    const state = useAuthStore.getState();
    expect(state.sessionStatus).toBe("pending-verification");
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  it("resolved unreachable-verify with a cached identity lands offline", async () => {
    useAuthStore.setState({
      user: mockUser,
      isAuthenticated: true,
      isLoading: false,
      sessionStatus: "local",
    });
    mockRefreshSession.mockResolvedValue(session("offline"));

    await useAuthStore.getState().refreshSession();

    const state = useAuthStore.getState();
    expect(state.sessionStatus).toBe("offline");
    expect(state.isAuthenticated).toBe(true);
    expect(state.user).toEqual(mockUser);
  });

  it("sign-out during an in-flight refresh discards the stale flight and re-verifies fresh", async () => {
    mockSignOut.mockResolvedValue(undefined);
    useAuthStore.setState({
      user: mockUser,
      isAuthenticated: true,
      isLoading: false,
      sessionStatus: "local",
    });
    let resolveStale!: (value: ReturnType<typeof session>) => void;
    let resolveFresh!: (value: ReturnType<typeof session>) => void;
    mockRefreshSession
      .mockReturnValueOnce(
        new Promise((resolve) => {
          resolveStale = resolve;
        }),
      )
      .mockReturnValueOnce(
        new Promise((resolve) => {
          resolveFresh = resolve;
        }),
      );

    const stale = useAuthStore.getState().refreshSession();
    await useAuthStore.getState().signOut();

    // The identity transition dropped the in-flight promise: the next caller
    // issues a new invoke instead of joining the previous account's verify.
    const fresh = useAuthStore.getState().refreshSession();
    expect(mockRefreshSession).toHaveBeenCalledTimes(2);

    resolveFresh(session("signed-out", false));
    await fresh;
    expect(useAuthStore.getState().sessionStatus).toBe("signed-out");

    // The abandoned flight settles late with the previous account's result —
    // it must be discarded, not applied over the new identity's state.
    resolveStale(session("verified"));
    await stale;
    const state = useAuthStore.getState();
    expect(state.user).toBeNull();
    expect(state.isAuthenticated).toBe(false);
    expect(state.sessionStatus).toBe("signed-out");
  });

  it("authoritative rejection resolves a pending session to signed-out", async () => {
    // The backend resolves an invalid/expired token as a signed-out session
    // (it does not reject), so that verdict must still land as signed-out.
    useAuthStore.setState({
      isLoading: false,
      sessionStatus: "pending-verification",
    });
    mockRefreshSession.mockResolvedValue(session("signed-out", false));

    await useAuthStore.getState().refreshSession();

    const state = useAuthStore.getState();
    expect(state.sessionStatus).toBe("signed-out");
    expect(state.isAuthenticated).toBe(false);
  });

  it("signOut clears the persisted provider capability cache", async () => {
    mockSignOut.mockResolvedValue(undefined);
    useAuthStore.setState({
      user: mockUser,
      isAuthenticated: true,
      isLoading: false,
    });
    useProviderCapabilities.setState({ claude: minimalCaps() });
    expect(
      localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY),
    ).not.toBeNull();

    await useAuthStore.getState().signOut();

    expect(useProviderCapabilities.getState().claude).toBeNull();
    expect(localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY)).toBeNull();
  });

  it("launch bootstrap into a cached user keeps the capability cache", async () => {
    useProviderCapabilities.setState({ claude: minimalCaps() });
    mockBootstrapSession.mockResolvedValue(session("local"));

    await useAuthStore.getState().bootstrapSession();

    expect(useProviderCapabilities.getState().claude).not.toBeNull();
    expect(
      localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY),
    ).not.toBeNull();
  });
});
