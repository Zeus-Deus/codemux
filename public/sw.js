/* No source code, credentials, API replies, or workspace data are cached. */
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) =>
  event.waitUntil(self.clients.claim()),
);
self.addEventListener("push", (event) => {
  let payload = {};
  try {
    payload = event.data?.json() ?? {};
  } catch {
    /* show a safe generic notification */
  }
  const url = new URL("/", self.location.origin);
  if (typeof payload.host === "string")
    url.searchParams.set("device", payload.host);
  if (typeof payload.workspace_id === "string")
    url.searchParams.set("workspace", payload.workspace_id);
  if (typeof payload.pane_id === "string")
    url.searchParams.set("pane", payload.pane_id);
  event.waitUntil(
    self.registration.showNotification(payload.title || "Codemux", {
      body: payload.body || "Your desktop has an update.",
      icon: "/icons/icon-256.png",
      tag: payload.tag || "codemux-update",
      data: { url: url.href },
    }),
  );
});
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  let url = new URL("/", self.location.origin);
  try {
    const candidate = new URL(event.notification.data?.url);
    if (candidate.origin === self.location.origin) url = candidate;
  } catch {
    /* safe home route */
  }
  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({
        type: "window",
        includeUncontrolled: true,
      });
      // Navigation always goes through authentication and device selection. Never
      // activate the same workspace ID on a different connected host.
      const target = windows.find((client) => client.url === url.href);
      if (target) return target.focus();
      return self.clients.openWindow(url.href);
    })(),
  );
});
self.addEventListener("fetch", (event) => {
  if (event.request.mode !== "navigate") return;
  event.respondWith(
    fetch(event.request).catch(
      () =>
        new Response(
          '<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover"><meta name="theme-color" content="#1c1917"><title>Codemux · Offline</title><body style="margin:0;background:#1c1917;color:#fafaf9;font:16px system-ui;display:grid;place-items:center;min-height:100dvh"><main style="max-width:28rem;padding:24px"><h1>You’re offline</h1><p>Reconnect to open Codemux. Your agents keep working on your desktop while it remains online.</p><a href="" style="display:inline-block;padding:14px 20px;color:#1c1917;background:#fafaf9;border-radius:12px;text-decoration:none">Try again</a></main></body></html>',
          {
            status: 503,
            headers: {
              "Content-Type": "text/html; charset=utf-8",
              "Cache-Control": "no-store",
            },
          },
        ),
    ),
  );
});
