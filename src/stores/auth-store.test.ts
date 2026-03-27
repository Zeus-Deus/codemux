import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock the Tauri commands before importing the store
vi.mock("@/tauri/commands", () => ({
  checkAuth: vi.fn(),
  startOauthFlow: vi.fn(),
  signinEmail: vi.fn(),
  signupEmail: vi.fn(),
  signOut: vi.fn(),
}));

import { useAuthStore } from "./auth-store";
import {
  checkAuth,
  signinEmail,
  signupEmail,
  signOut,
} from "@/tauri/commands";

const mockCheckAuth = vi.mocked(checkAuth);
const mockSigninEmail = vi.mocked(signinEmail);
const mockSignupEmail = vi.mocked(signupEmail);
const mockSignOut = vi.mocked(signOut);

beforeEach(() => {
  vi.clearAllMocks();
  // Reset store to initial state
  useAuthStore.setState({
    user: null,
    isAuthenticated: false,
    isLoading: true,
    isSigningIn: false,
    error: null,
    devBypass: false,
  });
});

describe("auth store", () => {
  it("starts in loading state", () => {
    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(true);
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  it("checkAuth → authenticated when user returned", async () => {
    const mockUser = { id: "u1", email: "a@b.com", name: "Test", image: null };
    mockCheckAuth.mockResolvedValue(mockUser);

    await useAuthStore.getState().checkAuth();

    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(false);
    expect(state.isAuthenticated).toBe(true);
    expect(state.user).toEqual(mockUser);
  });

  it("checkAuth → unauthenticated when null returned", async () => {
    mockCheckAuth.mockResolvedValue(null);

    await useAuthStore.getState().checkAuth();

    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(false);
    expect(state.isAuthenticated).toBe(false);
    expect(state.user).toBeNull();
  });

  it("checkAuth → dev bypass on error", async () => {
    mockCheckAuth.mockRejectedValue(new Error("Connection refused"));

    await useAuthStore.getState().checkAuth();

    const state = useAuthStore.getState();
    expect(state.isLoading).toBe(false);
    expect(state.isAuthenticated).toBe(true);
    expect(state.devBypass).toBe(true);
    expect(state.user?.id).toBe("dev-local");
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
});
