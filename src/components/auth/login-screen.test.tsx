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

const startOAuthFlow = vi.fn();
const signInEmail = vi.fn();
const signUpEmail = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
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

describe("LoginScreen", () => {
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
    expect(screen.getByPlaceholderText("Email")).toBeEnabled();
    expect(
      screen.getByRole("button", { name: /continue with github/i }),
    ).toBeEnabled();
  });

  it("toggles password visibility", () => {
    render(<LoginScreen />);
    expect(passwordInput().type).toBe("password");

    fireEvent.click(screen.getByRole("button", { name: "Show password" }));
    expect(passwordInput().type).toBe("text");

    fireEvent.click(screen.getByRole("button", { name: "Hide password" }));
    expect(passwordInput().type).toBe("password");
  });

  it("states the password rule and requires a name on sign-up only", () => {
    render(<LoginScreen />);
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
