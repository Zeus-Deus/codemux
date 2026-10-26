import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { DetailSection } from "@/components/settings/provider-parts";
import { cn } from "@/lib/utils";
import type { PluginInstallation } from "./contract";
import { buildPluginSettingsPatch, pluginFailure } from "./model";
import { platformUpdateSettings } from "./platform-adapter";
import {
  discardPluginSettings,
  editPluginSettings,
  pluginSettingsView,
  setPluginSettingsSaving,
  usePluginSettingsDraft,
  usePluginSettingsSaving,
} from "./plugin-settings-drafts";

/** Compact geometry on a phone, where buttons grow to 44px; the hit area stays that large. */
export const COMPACT_SWITCH = "min-h-0! after:-inset-y-3.5";

/** Whether the host holds a value for a secret. Secrets are never echoed, only marked. */
const hasStoredSecret = (installation: PluginInstallation, key: string) =>
  installation.storedSecrets?.includes(key) === true;

/**
 * A plugin's settings, edited in place. Edits apply only on Save, and only
 * the values the user changed; while nothing is edited the form follows the
 * stored values, so a change made elsewhere shows up instead of a stale one.
 */
export function PlatformPluginSettingsForm({ installation }: { installation: PluginInstallation }) {
  const { manifest, installationId } = installation;
  const idPrefix = useId();
  const entry = usePluginSettingsDraft(installation);
  const { base, draft, clearedSecrets } = pluginSettingsView(installation, entry);
  const dirty = entry !== null;
  const [errors, setErrors] = useState<Readonly<Record<string, string>>>({});
  const [error, setError] = useState<string | null>(null);
  const saving = usePluginSettingsSaving(installationId);

  const edit = (key: string, value: string | boolean, cleared = clearedSecrets) => {
    editPluginSettings(installation, { draft: { ...draft, [key]: value }, clearedSecrets: cleared });
    setErrors(({ [key]: _gone, ...rest }) => rest);
    setError(null);
  };
  const withCleared = (key: string, on: boolean) => {
    const next = new Set(clearedSecrets);
    if (on) next.add(key);
    else next.delete(key);
    return next;
  };
  const revert = () => {
    discardPluginSettings(installationId);
    setErrors({});
    setError(null);
  };
  const save = async () => {
    const patch = buildPluginSettingsPatch(manifest.settings, base, draft, clearedSecrets);
    if (!patch.ok) {
      setErrors(patch.errors);
      return;
    }
    if (Object.keys(patch.values).length === 0) {
      revert();
      return;
    }
    const submitted = entry;
    setPluginSettingsSaving(installationId, true);
    setError(null);
    try {
      await platformUpdateSettings(installationId, patch.values);
      discardPluginSettings(installationId, submitted);
    } catch (cause) {
      setError(`Couldn't save: ${pluginFailure(cause).message}`);
    } finally {
      setPluginSettingsSaving(installationId, false);
    }
  };

  return (
    <form
      data-plugin-settings-form={installationId}
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <DetailSection title="Settings" action={<span className="text-label text-muted-foreground/70">On this computer</span>}>
        <div className="grid gap-4">
          {manifest.settings.map((setting) => {
            const id = `${idPrefix}-${setting.key}`;
            const value = draft[setting.key];
            const fieldError = errors[setting.key];
            const describedBy = fieldError || setting.description ? `${id}-hint` : undefined;
            const hint = fieldError ? (
              <p id={`${id}-hint`} className="text-label text-destructive" role="alert">
                {fieldError}
              </p>
            ) : setting.description ? (
              <p id={`${id}-hint`} className="text-label text-muted-foreground">
                {setting.description}
              </p>
            ) : null;
            if (setting.type === "boolean") {
              return (
                <div key={setting.key} className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p id={`${id}-label`} className="text-body-sm font-medium text-foreground">
                      {setting.label}
                    </p>
                    {hint}
                  </div>
                  <Switch
                    className={cn(COMPACT_SWITCH, "mt-0.5")}
                    aria-labelledby={`${id}-label`}
                    aria-describedby={describedBy}
                    disabled={saving}
                    checked={value === true}
                    onCheckedChange={(checked) => edit(setting.key, checked)}
                  />
                </div>
              );
            }
            const stored = setting.type === "secret" && hasStoredSecret(installation, setting.key);
            const cleared = clearedSecrets.has(setting.key);
            return (
              <div key={setting.key} className="grid gap-1.5">
                <Label htmlFor={id}>{setting.label}</Label>
                <div className="flex min-w-0 items-center gap-2">
                  <Input
                    id={id}
                    className="min-w-0 flex-1"
                    type={setting.type === "secret" ? "password" : "text"}
                    inputMode={setting.type === "number" ? "decimal" : undefined}
                    autoComplete="off"
                    spellCheck={false}
                    disabled={saving}
                    aria-invalid={fieldError ? true : undefined}
                    aria-describedby={describedBy}
                    placeholder={stored && !cleared ? "Saved. Type to replace." : undefined}
                    value={typeof value === "string" ? value : ""}
                    onChange={(event) =>
                      edit(setting.key, event.target.value, setting.type === "secret" ? withCleared(setting.key, false) : clearedSecrets)
                    }
                  />
                  {stored ? (
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      disabled={saving}
                      aria-pressed={cleared}
                      aria-label={cleared ? `Keep the saved ${setting.label}` : `Delete the saved ${setting.label}`}
                      onClick={() => edit(setting.key, "", withCleared(setting.key, !cleared))}
                    >
                      {cleared ? "Keep saved" : "Delete saved"}
                    </Button>
                  ) : null}
                </div>
                {cleared ? (
                  <p className="text-label text-muted-foreground">The saved value is deleted when you save.</p>
                ) : null}
                {hint}
              </div>
            );
          })}
          {error ? (
            <p className="text-body-sm text-destructive" role="alert">
              {error}
            </p>
          ) : null}
          <div className="flex flex-wrap items-center justify-end gap-2">
            {dirty ? (
              <span className="me-auto text-label text-muted-foreground" role="status">
                Unsaved changes
              </span>
            ) : null}
            <Button type="button" size="sm" variant="ghost" disabled={!dirty || saving} onClick={revert}>
              Revert
            </Button>
            <Button type="submit" size="sm" disabled={!dirty || saving}>
              {saving ? "Saving…" : "Save"}
            </Button>
          </div>
        </div>
      </DetailSection>
    </form>
  );
}
