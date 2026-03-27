import { useState } from "react";
import { Github, Loader2, Mail } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useAuthStore } from "@/stores/auth-store";
import { forgotPassword } from "@/tauri/commands";

type View = "signin" | "signup" | "forgot-password" | "verify-email";

export function LoginScreen() {
  const isLoading = useAuthStore((s) => s.isLoading);
  const isSigningIn = useAuthStore((s) => s.isSigningIn);
  const error = useAuthStore((s) => s.error);
  const startOAuthFlow = useAuthStore((s) => s.startOAuthFlow);
  const signInEmail = useAuthStore((s) => s.signInEmail);
  const signUpEmail = useAuthStore((s) => s.signUpEmail);
  const clearError = useAuthStore((s) => s.clearError);

  const [view, setView] = useState<View>("signin");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [name, setName] = useState("");
  const [resetSent, setResetSent] = useState(false);
  const [resetLoading, setResetLoading] = useState(false);

  // Startup loading state — pulsing logo
  if (isLoading) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-background">
        <div className="text-xl font-semibold text-foreground animate-pulse opacity-80">
          codemux
        </div>
      </div>
    );
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (view === "signup") {
      await signUpEmail(email, password, name);
      if (!useAuthStore.getState().error) {
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
  };

  const isEmailNotVerified =
    error?.toLowerCase().includes("email not verified");

  // ─── Verify Email View ───────────────────────────────────────
  if (view === "verify-email") {
    return (
      <div className="flex flex-col h-screen w-screen bg-background">
        <div className="h-8 w-full shrink-0" data-tauri-drag-region />
        <div className="flex flex-1 items-center justify-center">
          <div className="flex flex-col items-center w-full max-w-sm px-6">
            <div className="mb-6">
              <span className="text-xl font-semibold text-foreground">
                codemux
              </span>
            </div>
            <Mail className="h-10 w-10 text-muted-foreground mb-4" />
            <h2 className="text-sm font-medium text-foreground mb-2">
              Check your email
            </h2>
            <p className="text-xs text-muted-foreground text-center mb-6">
              We sent a verification link to{" "}
              <span className="text-foreground">{email}</span>. Click the link
              to verify your account.
            </p>
            <Button
              className="w-full"
              size="lg"
              onClick={() => switchView("signin")}
            >
              I've verified my email
            </Button>
            <button
              type="button"
              className="mt-4 text-xs text-muted-foreground hover:text-foreground transition-colors"
              onClick={() => switchView("signin")}
            >
              Back to sign in
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ─── Forgot Password View ───────────────────────────────────
  if (view === "forgot-password") {
    return (
      <div className="flex flex-col h-screen w-screen bg-background">
        <div className="h-8 w-full shrink-0" data-tauri-drag-region />
        <div className="flex flex-1 items-center justify-center">
          <div className="flex flex-col items-center w-full max-w-sm px-6">
            <div className="mb-6">
              <span className="text-xl font-semibold text-foreground">
                codemux
              </span>
            </div>
            <div className="text-center mb-6">
              <p className="text-sm text-muted-foreground">
                Reset your password
              </p>
            </div>
            {resetSent ? (
              <>
                <p className="text-sm text-muted-foreground text-center mb-6">
                  If that email exists, we sent a reset link.
                </p>
                <Button
                  className="w-full"
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
                  className="w-full"
                  size="lg"
                  disabled={resetLoading}
                >
                  {resetLoading && (
                    <Loader2 className="h-4 w-4 animate-spin mr-1.5" />
                  )}
                  Send reset link
                </Button>
              </form>
            )}
            {!resetSent && (
              <button
                type="button"
                className="mt-4 text-xs text-muted-foreground hover:text-foreground transition-colors"
                onClick={() => switchView("signin")}
              >
                Back to sign in
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  // ─── Sign In / Sign Up View ─────────────────────────────────
  return (
    <div className="flex flex-col h-screen w-screen bg-background">
      {/* Draggable title bar area */}
      <div className="h-8 w-full shrink-0" data-tauri-drag-region />

      <div className="flex flex-1 items-center justify-center">
        <div className="flex flex-col items-center w-full max-w-sm px-6">
          {/* Logo / App name */}
          <div className="mb-6">
            <span className="text-xl font-semibold text-foreground">
              codemux
            </span>
          </div>

          {/* Subtitle */}
          <div className="text-center mb-6">
            <p className="text-sm text-muted-foreground">
              {view === "signin"
                ? "Sign in to get started"
                : "Create your account"}
            </p>
          </div>

          {/* GitHub OAuth */}
          <Button
            variant="outline"
            size="lg"
            className="w-full gap-2.5 mb-4"
            onClick={() => startOAuthFlow()}
            disabled={isSigningIn}
          >
            {isSigningIn ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Github className="h-4 w-4" />
            )}
            Continue with GitHub
          </Button>

          {/* Divider */}
          <div className="relative w-full mb-4">
            <div className="absolute inset-0 flex items-center">
              <span className="w-full border-t border-border" />
            </div>
            <div className="relative flex justify-center">
              <span className="bg-background px-2 text-xs text-muted-foreground">
                or
              </span>
            </div>
          </div>

          {/* Email/password form */}
          <form onSubmit={handleSubmit} className="w-full space-y-3">
            {view === "signup" && (
              <Input
                type="text"
                placeholder="Name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                disabled={isSigningIn}
                autoComplete="name"
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
            <Input
              type="password"
              placeholder="Password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              disabled={isSigningIn}
              autoComplete={
                view === "signup" ? "new-password" : "current-password"
              }
              required
            />

            {/* Forgot password link */}
            {view === "signin" && (
              <div className="flex justify-end">
                <button
                  type="button"
                  className="text-xs text-muted-foreground hover:text-primary transition-colors"
                  onClick={() => switchView("forgot-password")}
                >
                  Forgot password?
                </button>
              </div>
            )}

            {/* Error message */}
            {error &&
              (isEmailNotVerified ? (
                <div className="text-center space-y-1">
                  <p className="text-sm text-muted-foreground">
                    Your email hasn't been verified yet.
                  </p>
                  <p className="text-sm text-muted-foreground">
                    Check your inbox for the verification link.
                  </p>
                </div>
              ) : (
                <p className="text-destructive text-sm text-center">{error}</p>
              ))}

            <Button
              type="submit"
              className="w-full"
              size="lg"
              disabled={isSigningIn}
            >
              {isSigningIn && (
                <Loader2 className="h-4 w-4 animate-spin mr-1.5" />
              )}
              {view === "signin" ? "Sign in" : "Create account"}
            </Button>
          </form>

          {/* Toggle sign-in / sign-up */}
          <button
            type="button"
            className="mt-4 text-xs text-muted-foreground hover:text-foreground transition-colors"
            onClick={() =>
              switchView(view === "signin" ? "signup" : "signin")
            }
          >
            {view === "signin"
              ? "Don't have an account? Sign up"
              : "Already have an account? Sign in"}
          </button>
        </div>
      </div>
    </div>
  );
}
