import type { ApnsEnvironment, Category, Platform } from "./types";

export interface DeviceRecord {
  deviceId: string;
  platform: Platform;
  token: string;
  appId: string;
  /** Always set for iOS, always null for Android. */
  apnsEnvironment: ApnsEnvironment | null;
  /** Hex sha256 of the send key, which the phone hands to its desktops and
   *  which only authorizes sending. The key itself is never stored. */
  sendKeyHash: string;
  /** Hex sha256 of the manage key, which only the phone holds: it alone may
   *  change the token or delete the device. */
  manageKeyHash: string;
  /** Categories the phone muted. Enforced here so an opt-out holds even
   *  while the desktops that send them are out of the phone's reach. */
  mutedCategories: Category[];
  createdAt: Date;
  updatedAt: Date;
}

export interface PushRelayStore {
  create(record: DeviceRecord): Promise<void>;
  get(deviceId: string): Promise<DeviceRecord | undefined>;
  /** Leaves apnsEnvironment unchanged when omitted. */
  updateToken(deviceId: string, token: string, apnsEnvironment?: ApnsEnvironment): Promise<void>;
  setMutedCategories(deviceId: string, muted: Category[]): Promise<void>;
  delete(deviceId: string): Promise<void>;
  /** Delete only while the device still delivers to `token`; true when it
   *  did. SQL: `DELETE … WHERE device_id = $1 AND token = $2`. */
  deleteIfToken(deviceId: string, token: string): Promise<boolean>;
}

export function createMemoryStore(options: { now?: () => number } = {}): PushRelayStore {
  const now = options.now ?? Date.now;
  const records = new Map<string, DeviceRecord>();
  return {
    async create(record) {
      if (records.has(record.deviceId)) throw new Error("device already exists");
      records.set(record.deviceId, { ...record });
    },
    async get(deviceId) {
      const record = records.get(deviceId);
      return record && { ...record, mutedCategories: [...record.mutedCategories] };
    },
    async updateToken(deviceId, token, apnsEnvironment) {
      const record = records.get(deviceId);
      if (!record) return;
      record.token = token;
      if (apnsEnvironment) record.apnsEnvironment = apnsEnvironment;
      record.updatedAt = new Date(now());
    },
    async setMutedCategories(deviceId, muted) {
      const record = records.get(deviceId);
      if (!record) return;
      record.mutedCategories = [...muted];
      record.updatedAt = new Date(now());
    },
    async delete(deviceId) {
      records.delete(deviceId);
    },
    async deleteIfToken(deviceId, token) {
      if (records.get(deviceId)?.token !== token) return false;
      return records.delete(deviceId);
    },
  };
}

export type RateLimitResult = { allowed: true } | { allowed: false; retryAfterSeconds: number };

export interface RateLimiter {
  hit(key: string, limit: number, windowMs: number): Promise<RateLimitResult>;
}

/** Fixed-window counters. Single-process only; swap for Redis/Postgres when scaled out. */
export function createMemoryRateLimiter(options: { now?: () => number } = {}): RateLimiter {
  const now = options.now ?? Date.now;
  const windows = new Map<string, { resetAt: number; count: number }>();
  return {
    async hit(key, limit, windowMs) {
      const t = now();
      if (windows.size > 10_000) {
        for (const [k, w] of windows) if (w.resetAt <= t) windows.delete(k);
      }
      let window = windows.get(key);
      if (!window || window.resetAt <= t) {
        window = { resetAt: t + windowMs, count: 0 };
        windows.set(key, window);
      }
      if (window.count >= limit) {
        return { allowed: false, retryAfterSeconds: Math.max(1, Math.ceil((window.resetAt - t) / 1000)) };
      }
      window.count++;
      return { allowed: true };
    },
  };
}
