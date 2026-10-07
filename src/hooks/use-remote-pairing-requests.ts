import { useEffect } from "react";

import { isRemoteClient } from "@/components/remote/is-remote-client";
import {
  describeDevice,
  newlyPendingSessionIds,
} from "@/components/settings/remote-access-utils";
import { toast } from "@/lib/toast";
import { onWebRemoteStateChanged } from "@/remote/web-remote-events";
import {
  webRemoteApproveSession,
  webRemoteRejectSession,
  webRemoteStatus,
} from "@/tauri/commands";
import type { WebRemoteSessionView, WebRemoteStatus } from "@/tauri/types";

const toastId = (sessionId: string) => `remote-pairing-${sessionId}`;

/** "Linux · Chrome" → "Chrome on Linux": prose wraps cleanly in a narrow
 *  toast, where a "·" list leaves a dangling separator at the line end. */
function platformPhrase(platform: string): string {
  const [os, browser] = platform.split(" · ");
  return browser ? `${browser} on ${os}` : os;
}

function askToApprove(session: WebRemoteSessionView): void {
  const device = describeDevice(session.name, session.user_agent);
  const how =
    session.source === "account"
      ? "Signed in with your Codemux account"
      : "Used a pairing link";
  // A browser names itself after its platform ("Chrome on Linux"), so only
  // repeat the platform when the name says something else.
  const phrase = device.platform ? platformPhrase(device.platform) : "";
  const platform = phrase === device.title ? "" : phrase;
  toast.info(`${device.title} wants to connect`, {
    id: toastId(session.id),
    // The browser waits until someone answers, so the question stays up.
    duration: Infinity,
    description: platform ? `${platform}. ${how}.` : `${how}.`,
    action: {
      label: "Approve",
      onClick: () => {
        webRemoteApproveSession(session.id)
          .then((status) => {
            // Approving a request that was rejected or withdrawn meanwhile
            // changes nothing, so only report what actually happened.
            if (status.sessions.some((s) => s.id === session.id && s.approved)) {
              toast.success(`${device.title} can now use this desktop.`);
            } else {
              toast.error(`${device.title} is no longer waiting to connect.`);
            }
          })
          .catch((err) =>
            toast.error(`Couldn't approve ${device.title}: ${String(err)}`),
          );
      },
    },
    cancel: {
      label: "Reject",
      onClick: () => {
        webRemoteRejectSession(session.id).catch((err) =>
          toast.error(`Couldn't reject ${device.title}: ${String(err)}`),
        );
      },
    },
  });
}

/**
 * Ask on the desktop, wherever the user is, when a browser asks to connect.
 * Mounted once in the app shell so a waiting phone is never stuck behind a
 * closed Settings page. Requests that were already pending at launch stay in
 * Settings → Remote Access → Paired browsers; only new arrivals interrupt.
 * Each question closes itself once the request is answered anywhere.
 *
 * Desktop only: a remote browser can't approve other browsers.
 */
export function useRemotePairingRequests(): void {
  useEffect(() => {
    if (isRemoteClient()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // Set once the launch snapshot is in (or failed to load).
    let seeded = false;
    // Null until a first status is known, so requests from before launch are
    // seen as known rather than new. If the snapshot can't be loaded, the
    // first event becomes that baseline instead.
    let previous: WebRemoteSessionView[] | null = null;
    // Events that beat the first snapshot, replayed once it lands so a
    // request that arrives during startup is still announced.
    let early: WebRemoteStatus[] = [];
    const asked = new Set<string>();

    const apply = (status: WebRemoteStatus) => {
      for (const id of asked) {
        if (status.sessions.some((s) => s.id === id && !s.approved)) continue;
        asked.delete(id);
        toast.dismiss(toastId(id));
      }
      if (previous) {
        const fresh = new Set(newlyPendingSessionIds(previous, status.sessions));
        for (const session of status.sessions) {
          if (!fresh.has(session.id)) continue;
          asked.add(session.id);
          askToApprove(session);
        }
      }
      previous = status.sessions;
    };

    const seed = (status: WebRemoteStatus | null) => {
      if (disposed || seeded) return;
      seeded = true;
      if (status) apply(status);
      for (const event of early) apply(event);
      early = [];
    };
    webRemoteStatus().then(seed, () => seed(null));
    onWebRemoteStateChanged((status) => {
      if (seeded) apply(status);
      else early.push(status);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      disposed = true;
      unlisten?.();
      for (const id of asked) toast.dismiss(toastId(id));
    };
  }, []);
}
