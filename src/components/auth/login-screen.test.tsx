import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { LoginScreen } from "./login-screen";

const mocks = vi.hoisted(() => ({
  startOAuthFlow: vi.fn(), signInEmail: vi.fn(), signUpEmail: vi.fn(),
  clearError: vi.fn(), start: vi.fn(), cancel: vi.fn(), refresh: vi.fn(),
  enterLocal: vi.fn(), acknowledgeWelcome: vi.fn(), disconnect: vi.fn(),
}));
vi.mock("@/components/layout/window-chrome", () => ({ WindowChrome: () => null }));
vi.mock("@/stores/auth-store", () => ({ useAuthStore: (selector: (state: unknown) => unknown) => selector({
  isLoading: false, isSigningIn: false, error: null, ...mocks,
}) }));
vi.mock("@/stores/chatgpt-store", () => ({ useChatGptStore: (selector: (state: unknown) => unknown) => selector({
  status: { phase: "disconnected", attemptId: null, email: null, error: null,
    profiles: [], activeProfileId: null, welcomePending: false, installed: true },
  loading: false, busy: false, error: null, localMode: false, ...mocks,
}) }));
vi.mock("@/tauri/commands", () => ({ forgotPassword: vi.fn() }));
vi.mock("@/tauri/events", () => ({ onChatGptStatusChanged: vi.fn().mockResolvedValue(() => {}) }));

afterEach(() => { cleanup(); vi.clearAllMocks(); });
describe("LoginScreen ChatGPT entry", () => {
  it("offers ChatGPT plan setup separately from cloud account sign-in", () => {
    render(<LoginScreen />);
    const connect = screen.getByRole("button", { name: "Continue with ChatGPT" });
    expect(screen.getByText(/GitHub or email connects your CodeMux account for sync/i)).toBeTruthy();
    fireEvent.click(connect);
    expect(mocks.start).toHaveBeenCalledWith(null);
    expect(mocks.startOAuthFlow).not.toHaveBeenCalled();
  });
});
