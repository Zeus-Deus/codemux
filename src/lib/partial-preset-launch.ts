export interface PartialPresetLaunchResponse {
  confirmed_tab_ids: string[];
  /** Confirmed PTY writes, including tabs that closed before the response. */
  confirmed_commands?: number;
  total_commands: number;
  error: string;
}

/** A later preset command failed after earlier tabs received their commands. */
export class PartialPresetLaunchError extends Error {
  readonly confirmedTabIds: string[];
  readonly confirmedCommands: number;
  readonly totalCommands: number;

  constructor(result: PartialPresetLaunchResponse) {
    const confirmedCommands = result.confirmed_commands ?? result.confirmed_tab_ids.length;
    super(`Preset partly started (${confirmedCommands} of ${result.total_commands} commands): ${result.error}`);
    this.name = "PartialPresetLaunchError";
    this.confirmedTabIds = result.confirmed_tab_ids;
    this.confirmedCommands = confirmedCommands;
    this.totalCommands = result.total_commands;
  }
}
