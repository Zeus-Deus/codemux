export interface PartialPresetLaunchResponse {
  confirmed_tab_ids: string[];
  total_commands: number;
  error: string;
}

/** A later preset command failed after earlier tabs received their commands. */
export class PartialPresetLaunchError extends Error {
  readonly confirmedTabIds: string[];
  readonly totalCommands: number;

  constructor(result: PartialPresetLaunchResponse) {
    super(`Preset partly started (${result.confirmed_tab_ids.length} of ${result.total_commands} commands): ${result.error}`);
    this.name = "PartialPresetLaunchError";
    this.confirmedTabIds = result.confirmed_tab_ids;
    this.totalCommands = result.total_commands;
  }
}
