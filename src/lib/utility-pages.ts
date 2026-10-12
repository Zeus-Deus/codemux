/**
 * Pages that open in the main area while the sidebar stays put —
 * Automations, Devices, Pull requests and Usage. The sidebar footer swaps to a
 * Back button while one is open. At most one is open at a time.
 */
export type UtilityPage = "automations" | "devices" | "pull-requests" | "usage";

interface UtilityPageFlags {
  showAutomations: boolean;
  showDevices: boolean;
  showPullRequests: boolean;
  showUsage: boolean;
}

export function selectUtilityPage(state: UtilityPageFlags): UtilityPage | null {
  if (state.showAutomations) return "automations";
  if (state.showDevices) return "devices";
  if (state.showPullRequests) return "pull-requests";
  if (state.showUsage) return "usage";
  return null;
}
