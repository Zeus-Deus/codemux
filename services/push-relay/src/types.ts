export type Platform = "ios" | "android";
export type ApnsEnvironment = "sandbox" | "production";

export const CATEGORIES = ["approval", "input", "complete", "failure", "pr", "test"] as const;
export type Category = (typeof CATEGORIES)[number];

/** A validated notification, ready for a platform sender. */
export interface PushMessage {
  title: string;
  body: string;
  category: Category;
  collapseKey?: string;
  data: Record<string, string>;
  ttlSeconds: number;
}

export interface PushTarget {
  token: string;
  appId: string;
  apnsEnvironment: ApnsEnvironment | null;
}

export type DeliveryResult =
  | { ok: true }
  | { ok: false; gone: boolean; status: number; reason?: string };

export type Deliver = (target: PushTarget, message: PushMessage) => Promise<DeliveryResult>;
