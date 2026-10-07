/**
 * The selection a line reveal lands on, as document offsets. `column` and
 * `endColumn` are 1-based and `endColumn` is exclusive, so a search hit
 * selects exactly the matched text; without `endColumn` the cursor is
 * collapsed at `column`. Both ends are clamped to the line.
 */
export function revealSelection(
  line: { from: number; to: number },
  column: number | undefined,
  endColumn: number | undefined,
): { anchor: number; head: number } {
  const anchor = Math.min(line.to, line.from + Math.max(0, (column ?? 1) - 1));
  const head =
    endColumn != null
      ? Math.max(anchor, Math.min(line.to, line.from + endColumn - 1))
      : anchor;
  return { anchor, head };
}
