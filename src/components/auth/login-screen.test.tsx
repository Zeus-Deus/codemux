import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  forgotPassword: vi.fn(),
  resendVerificationEmail: vi.fn(),
}));
vi.mock("@/components/layout/window-chrome", () => ({
  WindowChrome: () => null,
}));

import {
  LoginScreen,
  MIN_PASSWORD_LENGTH,
  RESEND_COOLDOWN_SECONDS,
  RESEND_RATE_LIMIT_COOLDOWN_SECONDS,
} from "./login-screen";
import { useAuthStore } from "@/stores/auth-store";
import { resendVerificationEmail } from "@/tauri/commands";

import { useChatGptStore } from "@/stores/chatgpt-store";
import type { ChatGptStatus } from "@/tauri/chatgpt";

const startChatGpt = vi.fn();
const enterLocal = vi.fn();
const startOAuthFlow = vi.fn();
const signInEmail = vi.fn();
const signUpEmail = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  useChatGptStore.setState({
    status: null, loading: false, busy: false, error: null, subscriptionError: null,
    localMode: false, start: startChatGpt, enterLocal,
  });
  vi.mocked(resendVerificationEmail).mockResolvedValue(undefined);
  useAuthStore.setState({
    isLoading: false,
    isSigningIn: false,
    oauthPending: false,
    error: null,
    startOAuthFlow,
    signInEmail,
    signUpEmail,
  });
});

afterEach(cleanup);

function passwordInput() {
  return screen.getByPlaceholderText("Password") as HTMLInputElement;
}

function openEmailForm() {
  fireEvent.click(screen.getByRole("button", { name: "Continue with email" }));
}

function chatGptStatus(overrides: Partial<ChatGptStatus>): ChatGptStatus {
  return {
    phase: "disconnected", attemptId: null, email: null, error: null, profiles: [],
    activeProfileId: null, welcomePending: false, installed: true, ...overrides,
  };
}

describe("LoginScreen", () => {
  it("offers three sign-in buttons with the email form folded away", () => {
    render(<LoginScreen />);
    expect(screen.getByRole("heading", { name: "Many agents, one desk." })).toBeVisible();
    expect(screen.getByText("Sign in to sync threads and settings across your devices.")).toBeVisible();
    expect(screen.queryByPlaceholderText("Email")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Continue with ChatGPT" }));
    expect(startChatGpt).toHaveBeenCalledWith(null);
    expect(startOAuthFlow).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Continue with GitHub" }));
    expect(startOAuthFlow).toHaveBeenCalledOnce();

    openEmailForm();
    expect(screen.getByPlaceholderText("Email")).toHaveFocus();
    expect(screen.queryByRole("button", { name: "Continue with email" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sign in" })).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Continue without an account" }));
    expect(enterLocal).toHaveBeenCalledOnce();
  });

  it("returns a local user to the workbench", () => {
    useChatGptStore.setState({ localMode: true });
    render(<LoginScreen />);
    fireEvent.click(screen.getByRole("button", { name: "Back to workbench" }));
    expect(enterLocal).toHaveBeenCalledOnce();
  });

  it("shows a GitHub sign-in error while the email form is folded", () => {
    useAuthStore.setState({ error: "Sign-in timed out" });
    render(<LoginScreen />);
    expect(screen.getByRole("alert")).toHaveTextContent("Sign-in timed out");
    expect(screen.queryByPlaceholderText("Email")).not.toBeInTheDocument();
  });

  it("gives the column to a pending ChatGPT connection", () => {
    useChatGptStore.setState({ status: chatGptStatus({ phase: "pending", attemptId: "attempt" }) });
    render(<LoginScreen />);
    expect(screen.getByText("Finish in your browser")).toBeVisible();
    expect(screen.getByRole("button", { name: "Cancel sign-in" })).toBeVisible();
    expect(screen.queryByRole("button", { name: /^Continue with (GitHub|ChatGPT|email)$/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Continue without an account" })).toBeDisabled();
  });

  it("keeps account sign-in beside a connected ChatGPT plan", () => {
    useChatGptStore.setState({
      status: chatGptStatus({ phase: "connected", email: "dev@example.com" }),
    });
    render(<LoginScreen />);
    expect(screen.getByText("ChatGPT connected")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Continue to Codemux" }));
    expect(enterLocal).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Continue with GitHub" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Continue with email" })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Continue with ChatGPT" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Continue without an account" })).not.toBeInTheDocument();
  });

  it("still requires the Codex CLI note when it is missing", () => {
    useChatGptStore.setState({ status: chatGptStatus({ installed: false }) });
    render(<LoginScreen />);
    expect(screen.getByText(/Codex CLI is required/)).toBeVisible();
  });

  it("replaces the form with a cancellable wait while GitHub sign-in is open", () => {
    useAuthStore.setState({ oauthPending: true });
    render(<LoginScreen />);

    expect(screen.getByText("Waiting for GitHub")).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("Email")).not.toBeInTheDocument();
    // The live region announces the message, not the action labels.
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Waiting for GitHub");
    expect(status).not.toHaveTextContent(/reopen browser|cancel/i);

    fireEvent.click(screen.getByRole("button", { name: /reopen browser/i }));
    expect(startOAuthFlow).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(useAuthStore.getState().oauthPending).toBe(false);
    expect(
      screen.getByRole("button", { name: "Continue with email" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: /continue with github/i }),
    ).toBeEnabled();
  });

  it("toggles password visibility", () => {
    render(<LoginScreen />);
    openEmailForm();
    expect(passwordInput().type).toBe("password");

    fireEvent.click(screen.getByRole("button", { name: "Show password" }));
    expect(passwordInput().type).toBe("text");

    fireEvent.click(screen.getByRole("button", { name: "Hide password" }));
    expect(passwordInput().type).toBe("password");
  });

  it("states the password rule and requires a name on sign-up only", () => {
    render(<LoginScreen />);
    openEmailForm();
    expect(passwordInput()).not.toHaveAttribute("minlength");

    fireEvent.click(screen.getByRole("button", { name: /sign up/i }));

    expect(screen.getByPlaceholderText("Name")).toBeRequired();
    expect(passwordInput()).toHaveAttribute(
      "minlength",
      String(MIN_PASSWORD_LENGTH),
    );
    expect(passwordInput()).toHaveAccessibleDescription(
      `At least ${MIN_PASSWORD_LENGTH} characters`,
    );
  });

  it("offers to resend the verification email when sign-in finds it unverified", async () => {
    useAuthStore.setState({ error: "Email not verified" });
    render(<LoginScreen />);
    fireEvent.change(screen.getByPlaceholderText("Email"), {
      target: { value: "new@example.com" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Resend email" }));

    await waitFor(() =>
      expect(resendVerificationEmail).toHaveBeenCalledWith("new@example.com"),
    );
    expect(await screen.findByText(/check your inbox and spam/i)).toBeVisible();
    expect(
      screen.getByRole("button", { name: /resend email in \d+s/i }),
    ).toBeDisabled();
  });

  it("shows the resend error instead of claiming success", async () => {
    vi.mocked(resendVerificationEmail).mockRejectedValue(
      "Couldn't resend the verification email",
    );
    useAuthStore.setState({ error: "Email not verified" });
    render(<LoginScreen />);

    fireEvent.click(screen.getByRole("button", { name: "Resend email" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't resend the verification email",
    );
    expect(screen.getByRole("button", { name: "Resend email" })).toBeEnabled();
  });

  it("holds Resend for the rate-limit window after a 429", async () => {
    vi.mocked(resendVerificationEmail).mockRejectedValue(
      "Too many requests. Wait a minute, then try again.",
    );
    useAuthStore.setState({ error: "Email not verified" });
    render(<LoginScreen />);

    fireEvent.click(screen.getByRole("button", { name: "Resend email" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Too many requests",
    );
    const button = screen.getByRole("button", {
      name: /resend email in \d+s/i,
    });
    expect(button).toBeDisabled();
    // Longer than the success cooldown, matching "wait a minute" in the copy.
    const seconds = Number(button.textContent?.match(/(\d+)s/)?.[1]);
    expect(seconds).toBeGreaterThan(RESEND_COOLDOWN_SECONDS);
    expect(seconds).toBeLessThanOrEqual(RESEND_RATE_LIMIT_COOLDOWN_SECONDS);
  });

  it("holds Resend for a cooldown right after sign-up sent the first email", async () => {
    signUpEmail.mockResolvedValue(undefined);
    render(<LoginScreen />);
    openEmailForm();
    fireEvent.click(screen.getByRole("button", { name: /sign up/i }));
    fireEvent.change(screen.getByPlaceholderText("Name"), {
      target: { value: "Ada" },
    });
    fireEvent.change(screen.getByPlaceholderText("Email"), {
      target: { value: "ada@example.com" },
    });
    fireEvent.change(passwordInput(), { target: { value: "long-enough" } });

    fireEvent.click(screen.getByRole("button", { name: "Create account" }));

    expect(await screen.findByText("Check your email")).toBeVisible();
    expect(
      screen.getByRole("button", { name: /resend email in \d+s/i }),
    ).toBeDisabled();
  });
});
