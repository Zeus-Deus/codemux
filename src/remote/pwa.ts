export interface InstallPrompt extends Event {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: string }>;
}
let installPrompt: InstallPrompt | null = null;
export function pendingInstallPrompt() {
  return installPrompt;
}
export function clearInstallPrompt() {
  installPrompt = null;
}

/** Bootstrap outside React so the sign-in screen also gets phone-safe sizing. */
export function initializeBrowserApp() {
  if ("__TAURI_INTERNALS__" in window) return;
  window.addEventListener("beforeinstallprompt", (event) => {
    event.preventDefault();
    installPrompt = event as InstallPrompt;
  });
  window.addEventListener("appinstalled", clearInstallPrompt);
  document.documentElement.dataset.browserClient = "true";
  const media = matchMedia("(max-width: 1023px)");
  const update = () => {
    if (media.matches) document.documentElement.dataset.mobile = "true";
    else delete document.documentElement.dataset.mobile;
  };
  update();
  media.addEventListener("change", update);
  if (
    window.isSecureContext &&
    "serviceWorker" in navigator &&
    !import.meta.env.DEV
  ) {
    void navigator.serviceWorker
      .register("/sw.js", { scope: "/", updateViaCache: "none" })
      .catch((error) => console.warn("Web app registration failed", error));
  }
}
