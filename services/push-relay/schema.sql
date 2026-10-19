-- Device registrations for the Codemux push relay (see src/store.ts DeviceRecord).
-- send_key_hash is the hex sha256 of the capability key; the key itself is never stored.
CREATE TABLE IF NOT EXISTS push_relay_devices (
  device_id        text PRIMARY KEY,
  platform         text NOT NULL CHECK (platform IN ('ios', 'android')),
  token            text NOT NULL CHECK (char_length(token) BETWEEN 1 AND 4096),
  app_id           text NOT NULL,
  apns_environment text CHECK (apns_environment IN ('sandbox', 'production')),
  send_key_hash    char(64) NOT NULL,
  manage_key_hash  char(64) NOT NULL,
  muted_categories text[] NOT NULL DEFAULT '{}',
  created_at       timestamptz NOT NULL DEFAULT now(),
  updated_at       timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT push_relay_devices_apns_env_ios_only
    CHECK ((platform = 'ios') = (apns_environment IS NOT NULL))
);
