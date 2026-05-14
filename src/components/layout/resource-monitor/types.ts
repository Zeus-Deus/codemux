// Shared UI types for the resource monitor popover. The data shapes
// (snapshot / app / workspace / session metrics) come from the Rust
// backend and live in `@/tauri/types`.

export type UsageSeverity = "normal" | "elevated" | "high";

export type SortOption = "memory" | "cpu" | "name";

export interface UsageValues {
  cpu: number;
  memory: number;
}
