import { useEffect, useState, type ComponentProps, type ReactNode } from "react";
import { Eye, EyeOff, Github, Loader2, Mail } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { WindowChrome } from "@/components/layout/window-chrome";
import { cn } from "@/lib/utils";
import { useAuthStore } from "@/stores/auth-store";
import { forgotPassword, resendVerificationEmail } from "@/tauri/commands";
import wordmark from "@/assets/codemux-wordmark.svg";

type View = "signin" | "signup" | "forgot-password" | "verify-email";

export const RESEND_COOLDOWN_SECONDS = 30;
export const MIN_PASSWORD_LENGTH = 8;

const linkButtonClass =
  "mt-4 text-label text-muted-foreground hover:text-foreground transition-colors duration-150";
const primaryButtonClass =
  "w-full bg-foreground text-background hover:bg-foreground/90";

export function LoginScreen() {
  const isLoading = useAuthStore((s) => s.isLoading);
  const isSigningIn = useAuthStore((s) => s.isSigningIn);
  const oauthPending = useAuthStore((s) => s.oauthPending);
  const error = useAuthStore((s) => s.error);
  const startOAuthFlow = useAuthStore((s) => s.startOAuthFlow);
  const cancelOAuthFlow = useAuthStore((s) => s.cancelOAuthFlow);
  const signInEmail = useAuthStore((s) => s.signInEmail);
  const signUpEmail = useAuthStore((s) => s.signUpEmail);
  const clearError = useAuthStore((s) => s.clearError);

  const [view, setView] = useState<View>("signin");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [resetSent, setResetSent] = useState(false);
  const [resetLoading, setResetLoading] = useState(false);
  const resend = useResendVerification(email);

  // Startup loading state — pulsing logo
  if (isLoading) {
    return (
      <div className="relative flex h-screen w-screen items-center justify-center bg-background">
        <WindowChrome />
        <Wordmark className="opacity-80 motion-safe:animate-pulse" />
      </div>
    );
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (view === "signup") {
      await signUpEmail(email, password, name);
      if (!useAuthStore.getState().error) {
        // Sign-up just sent the first email; hold Resend for the same window.
        resend.startCooldown();
        setView("verify-email");
      }
    } else {
      await signInEmail(email, password);
    }
  };

  const handleForgotPassword = async (e: React.FormEvent) => {
    e.preventDefault();
    setResetLoading(true);
    try {
      await forgotPassword(email);
    } catch {
      // Always show success — don't leak user existence
    }
    setResetLoading(false);
    setResetSent(true);
  };

  const switchView = (v: View) => {
    setView(v);
    clearError();
    setResetSent(false);
    resend.reset();
  };

  const isEmailNotVerified =
    error?.toLowerCase().includes("email not verified");

  let body: ReactNode;
  if (oauthPending) {
    body = (
      <div role="status" className="flex w-full flex-col items-center text-center">
        <Loader2 className="size-4 text-muted-foreground mb-4 motion-safe:animate-spin" />
        <h2 className="text-body font-medium text-foreground mb-2">
          Waiting for GitHub
        </h2>
        <p className="text-label text-muted-foreground mb-6">
          Finish signing in in your browser. Codemux continues as soon as
          GitHub sends you back.
        </p>
        <Button
          variant="outline"
          size="lg"
          className="w-full gap-2.5"
          onClick={() => void startOAuthFlow()}
        >
          <Github className="size-4" />
          Reopen browser
        </Button>
        <button type="button" className={linkButtonClass} onClick={cancelOAuthFlow}>
          Cancel
        </button>
      </div>
    );
  } else if (view === "verify-email") {
    body = (
      <>
        <Mail className="size-10 text-muted-foreground mb-4" />
        <h2 className="text-body font-medium text-foreground mb-2">
          Check your email
        </h2>
        <p className="text-label text-muted-foreground text-center mb-6">
          We sent a verification link to{" "}
          <span className="text-foreground">{email}</span>. Click the link
          to verify your account.
        </p>
        <Button
          className={primaryButtonClass}
          size="lg"
          onClick={() => switchView("signin")}
        >
          I've verified my email
        </Button>
        <ResendVerification resend={resend} className="mt-3" />
      </>
    );
  } else if (view === "forgot-password") {
    body = (
      <>
        <div className="text-center mb-6">
          <p className="text-body text-muted-foreground">Reset your password</p>
        </div>
        {resetSent ? (
          <>
            <p className="text-body text-muted-foreground text-center mb-6">
              If that email exists, we sent a reset link.
            </p>
            <Button
              className={primaryButtonClass}
              size="lg"
              onClick={() => switchView("signin")}
            >
              Back to sign in
            </Button>
          </>
        ) : (
          <form onSubmit={handleForgotPassword} className="w-full space-y-3">
            <Input
              type="email"
              placeholder="Email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              disabled={resetLoading}
              autoComplete="email"
              required
            />
            <Button
              type="submit"
              className={primaryButtonClass}
              size="lg"
              disabled={resetLoading}
            >
              {resetLoading && (
                <Loader2 className="size-4 animate-spin mr-1.5" />
              )}
              Send reset link
            </Button>
          </form>
        )}
        {!resetSent && (
          <button
            type="button"
            className={linkButtonClass}
            onClick={() => switchView("signin")}
          >
            Back to sign in
          </button>
        )}
      </>
    );
  } else {
    const isSignup = view === "signup";
    body = (
      <>
        <div className="text-center mb-6">
          <p className="text-body text-muted-foreground">
            {isSignup ? "Create your account" : "Sign in to get started"}
          </p>
        </div>

        <Button
          variant="outline"
          size="lg"
          className="w-full gap-2.5 mb-4"
          onClick={() => void startOAuthFlow()}
          disabled={isSigningIn}
        >
          <Github className="size-4" />
          Continue with GitHub
        </Button>

        <div className="relative w-full mb-4">
          <div className="absolute inset-0 flex items-center">
            <span className="w-full border-t border-border" />
          </div>
          <div className="relative flex justify-center">
            <span className="bg-background px-2 text-label text-muted-foreground">
              or
            </span>
          </div>
        </div>

        <form onSubmit={handleSubmit} className="w-full space-y-3">
          {isSignup && (
            <Input
              type="text"
              placeholder="Name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={isSigningIn}
              autoComplete="name"
              required
            />
          )}
          <Input
            type="email"
            placeholder="Email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            disabled={isSigningIn}
            autoComplete="email"
            required
          />
          <div className="space-y-1.5">
            <PasswordInput
              placeholder="Password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={isSigningIn}
              autoComplete={isSignup ? "new-password" : "current-password"}
              // Only new passwords are held to the rule; existing accounts
              // may predate it and must still be able to sign in.
              minLength={isSignup ? MIN_PASSWORD_LENGTH : undefined}
              aria-describedby={isSignup ? "login-password-hint" : undefined}
              required
            />
            {isSignup && (
              <p id="login-password-hint" className="text-label text-muted-foreground">
                At least {MIN_PASSWORD_LENGTH} characters
              </p>
            )}
          </div>

          {!isSignup && (
            <div className="flex justify-end">
              <button
                type="button"
                className="text-label text-muted-foreground hover:text-foreground transition-colors duration-150"
                onClick={() => switchView("forgot-password")}
              >
                Forgot password?
              </button>
            </div>
          )}

          {error &&
            (isEmailNotVerified ? (
              <div className="flex flex-col items-center text-center space-y-1">
                <p className="text-body text-muted-foreground">
                  Your email hasn't been verified yet.
                </p>
                <p className="text-body text-muted-foreground">
                  Check your inbox for the verification link.
                </p>
                <ResendVerification resend={resend} />
              </div>
            ) : (
              <p className="text-destructive text-body text-center">{error}</p>
            ))}

          <Button
            type="submit"
            className={primaryButtonClass}
            size="lg"
            disabled={isSigningIn}
          >
            {isSigningIn && <Loader2 className="size-4 animate-spin mr-1.5" />}
            {isSignup ? "Create account" : "Sign in"}
          </Button>
        </form>

        <button
          type="button"
          className={linkButtonClass}
          onClick={() => switchView(isSignup ? "signin" : "signup")}
        >
          {isSignup
            ? "Already have an account? Sign in"
            : "Don't have an account? Sign up"}
        </button>
      </>
    );
  }

  return (
    <div className="relative flex flex-col h-screen w-screen bg-background">
      <WindowChrome />
      {/* Top spacer reserves room for the WindowChrome strip overlay so the
          centered content doesn't visually collide with the controls. */}
      <div className="h-8 w-full shrink-0" />

      <div className="flex flex-1 items-center justify-center">
        <div className="flex flex-col items-center w-full max-w-sm px-6">
          <Wordmark className="mb-6" />
          {/* Keyed so each view change replays the entrance instead of
              hard-cutting the layout. */}
          <div
            key={oauthPending ? "oauth" : view}
            data-testid="login-view"
            className="flex w-full flex-col items-center motion-safe:animate-in fade-in-0 slide-in-from-bottom-1 duration-150"
          >
            {body}
          </div>
        </div>
      </div>
    </div>
  );
}

/** The wordmark asset has a fixed light fill. Using it as a mask lets the
 *  theme's foreground paint it, so it stays legible on light themes. */
function Wordmark({ className }: { className?: string }) {
  const mask = `url("${wordmark}") center / contain no-repeat`;
  return (
    <div
      role="img"
      aria-label="Codemux"
      className={cn("h-8 aspect-[21/4] shrink-0 bg-foreground", className)}
      style={{ mask, WebkitMask: mask }}
    />
  );
}

function PasswordInput(props: Omit<ComponentProps<typeof Input>, "type">) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="relative">
      <Input {...props} type={visible ? "text" : "password"} className="pr-9" />
      <button
        type="button"
        aria-label={visible ? "Hide password" : "Show password"}
        aria-pressed={visible}
        disabled={props.disabled}
        onClick={() => setVisible((v) => !v)}
        className="absolute inset-y-0 right-0 flex w-9 items-center justify-center rounded-md text-muted-foreground transition-colors duration-100 hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
      >
        {visible ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
      </button>
    </div>
  );
}

type ResendStatus = "idle" | "sending" | "sent" | "error";

function useResendVerification(email: string) {
  const [status, setStatus] = useState<ResendStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [cooldown, setCooldown] = useState(0);

  useEffect(() => {
    if (cooldown <= 0) return;
    const timer = setTimeout(() => setCooldown((c) => c - 1), 1000);
    return () => clearTimeout(timer);
  }, [cooldown]);

  const send = async () => {
    setStatus("sending");
    setError(null);
    try {
      await resendVerificationEmail(email);
      setStatus("sent");
      setCooldown(RESEND_COOLDOWN_SECONDS);
    } catch (err) {
      setStatus("error");
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return {
    status,
    error,
    cooldown,
    send,
    startCooldown: () => setCooldown(RESEND_COOLDOWN_SECONDS),
    reset: () => {
      setStatus("idle");
      setError(null);
    },
  };
}

function ResendVerification({
  resend,
  className,
}: {
  resend: ReturnType<typeof useResendVerification>;
  className?: string;
}) {
  const { status, error, cooldown } = resend;
  return (
    <div className={cn("flex flex-col items-center gap-1", className)}>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        className="text-muted-foreground"
        disabled={status === "sending" || cooldown > 0}
        onClick={() => void resend.send()}
      >
        {status === "sending" && (
          <Loader2 className="size-3.5 motion-safe:animate-spin" />
        )}
        {cooldown > 0 ? (
          <span>
            Resend email in <span className="tabular-nums">{cooldown}s</span>
          </span>
        ) : (
          "Resend email"
        )}
      </Button>
      {status === "sent" && (
        <p role="status" className="text-label text-muted-foreground">
          Sent. Check your inbox and spam folder.
        </p>
      )}
      {status === "error" && (
        <p role="alert" className="text-label text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
