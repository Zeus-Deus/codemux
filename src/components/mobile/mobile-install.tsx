import { useEffect, useState } from "react";
import { Download, X } from "lucide-react";
import {
  currentSubscription,
  defaultCategories,
  disablePush,
  enablePush,
  isAppleMobile,
  isStandalone,
  pushUnavailableReason,
  updatePushCategories,
  type PushCategories,
} from "@/remote/push";
import { invoke } from "@tauri-apps/api/core";
import {
  pendingInstallPrompt,
  clearInstallPrompt,
  type InstallPrompt,
} from "@/remote/pwa";
export function MobileInstall({ compact = false }: { compact?: boolean }) {
  const [standalone, setStandalone] = useState(isStandalone);
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem("codemux.install.dismissed") === "true";
    } catch {
      return false;
    }
  });
  const [prompt, setPrompt] = useState<InstallPrompt | null>(
    pendingInstallPrompt,
  );
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [categories, setCategories] =
    useState<PushCategories>(defaultCategories);
  useEffect(() => {
    const capture = (e: Event) => {
      e.preventDefault();
      setPrompt(e as InstallPrompt);
    };
    const installed = () => {
      setStandalone(true);
      setPrompt(null);
    };
    let mounted = true;
    const refresh = () => {
      if (pushUnavailableReason()) {
        setEnabled(false);
        return;
      }
      void Promise.all([
        currentSubscription(),
        invoke<{ categories?: PushCategories }>("web_push_status"),
      ])
        .then(([sub, value]) => {
          if (!mounted) return;
          setEnabled(
            !!sub &&
              !!value.categories &&
              Notification.permission === "granted",
          );
          if (value.categories) setCategories(value.categories);
        })
        .catch(() => {
          if (mounted) setEnabled(false);
        });
    };
    refresh();
    window.addEventListener("beforeinstallprompt", capture);
    window.addEventListener("appinstalled", installed);
    window.addEventListener("focus", refresh);
    return () => {
      mounted = false;
      window.removeEventListener("beforeinstallprompt", capture);
      window.removeEventListener("appinstalled", installed);
      window.removeEventListener("focus", refresh);
    };
  }, []);
  if (compact && (dismissed || standalone)) return null;
  const reason = pushUnavailableReason();
  async function toggle() {
    setBusy(true);
    setMessage("");
    try {
      if (enabled) await disablePush();
      else await enablePush(categories);
      setEnabled(!enabled);
      setMessage(
        enabled
          ? "Notifications turned off for this desktop."
          : "Notifications enabled, including when this app is closed. Your desktop must remain running and online.",
      );
    } catch (error) {
      setMessage(String(error instanceof Error ? error.message : error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className={
        compact ? "mobile-install" : "mobile-install mobile-install-settings"
      }
    >
      {!standalone && (
        <>
          <div className="flex items-center gap-2">
            <Download size={18} />
            <strong className="flex-1">Add Codemux to your Home Screen</strong>
            {compact && (
              <button
                aria-label="Dismiss installation tip"
                onClick={() => {
                  setDismissed(true);
                  try {
                    localStorage.setItem("codemux.install.dismissed", "true");
                  } catch {
                    /* optional */
                  }
                }}
              >
                <X size={16} />
              </button>
            )}
          </div>
          <p>
            {isAppleMobile()
              ? "Open Share → Add to Home Screen. If offered, turn on Open as Web App. Then launch Codemux from its icon for a full-screen experience and notifications."
              : "Choose Install app or Add to Home screen in your browser menu, then launch Codemux from its icon."}
          </p>
          {prompt && (
            <button
              className="mobile-primary"
              onClick={() => {
                void prompt
                  .prompt()
                  .then(() => prompt.userChoice)
                  .then(() => {
                    clearInstallPrompt();
                    setPrompt(null);
                  })
                  .catch(() =>
                    setMessage("Use your browser menu to install Codemux."),
                  );
              }}
            >
              Install Codemux
            </button>
          )}
        </>
      )}
      {!compact && (
        <>
          <h3 className="mt-6 font-semibold">Background notifications</h3>
          <p>
            Get notified when agents need you, finish work, or fail. These
            preferences apply to this desktop.
          </p>
          {(Object.keys(categories) as (keyof PushCategories)[]).map((key) => (
            <label key={key} className="flex min-h-11 items-center gap-3">
              <input
                type="checkbox"
                checked={categories[key]}
                disabled={busy}
                onChange={async (e) => {
                  const next = { ...categories, [key]: e.target.checked };
                  setBusy(true);
                  try {
                    if (enabled) await updatePushCategories(next);
                    setCategories(next);
                  } catch (error) {
                    setMessage(String(error));
                  } finally {
                    setBusy(false);
                  }
                }}
              />
              {
                {
                  attention: "Questions and approvals",
                  complete: "Ready for review",
                  failure: "Failures",
                }[key]
              }
            </label>
          ))}
          {reason && <p>{reason}</p>}
          <button
            className="mobile-primary"
            disabled={busy || !!reason}
            onClick={() => void toggle()}
          >
            {busy
              ? "Updating…"
              : enabled
                ? "Turn off notifications"
                : "Enable notifications"}
          </button>
          {enabled && (
            <button
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void invoke("web_push_test")
                  .then(() => setMessage("Test notification sent."))
                  .catch((error) => setMessage(String(error)))
                  .finally(() => setBusy(false));
              }}
            >
              Send test notification
            </button>
          )}
          <p role="status" aria-live="polite">
            {message}
          </p>
        </>
      )}
    </section>
  );
}
