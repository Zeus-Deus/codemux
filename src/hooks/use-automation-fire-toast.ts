import { useCallback } from "react";

import { useTauriEvent } from "./use-tauri-event";
import { onAutomationFire } from "@/tauri/events";
import { automationsGet, type AutomationRunView } from "@/tauri/commands";
import { toast } from "@/lib/toast";

/**
 * Surfaces an ambient toast each time the scheduler fires an automation.
 *
 * The run row is already persisted by the backend before the
 * `automations://fire` event is emitted — this hook is purely the
 * "your automation just ran" signal. The automation name is fetched
 * lazily so the event payload can stay the lean run row.
 */
export function useAutomationFireToast() {
  const handleFire = useCallback((run: AutomationRunView) => {
    void automationsGet(run.automation_id)
      .then((automation) => {
        toast.info(`Automation "${automation.name}" fired`);
      })
      .catch(() => {
        toast.info("An automation fired");
      });
  }, []);

  useTauriEvent(onAutomationFire, handleFire, [handleFire]);
}
