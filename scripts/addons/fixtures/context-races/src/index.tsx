import {
  definePlugin,
  Heading,
  PluginError,
  Stack,
  Text,
  useEffect,
  useReducer,
} from "@codemux/plugin-sdk";

// CI-only: each command waits past a context change and then tries to append
// to the draft it was started from. Outcomes are shown in a panel rather than
// notifications so the three-per-minute notification limit cannot hide them.
const cases = ["typing", "workspace", "thread", "replacement", "disable"];
const outcomes: string[] = [];
const listeners = new Set<() => void>();
function record(line: string) {
  outcomes.push(line);
  if (outcomes.length > 12) outcomes.shift();
  for (const listener of listeners) listener();
}
function Status() {
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    listeners.add(rerender);
    return () => {
      listeners.delete(rerender);
    };
  }, []);
  return (
    <Stack spacing="sm">
      <Heading>Context race status</Heading>
      <Text color="muted">CI status ready</Text>
      {outcomes.map((line) => (
        <Text>{`CI ${line}`}</Text>
      ))}
    </Stack>
  );
}
export default definePlugin({
  activate(ctx) {
    ctx.panels.register("status", () => <Status />);
    ctx.commands.register("status", (context) =>
      ctx.panels.open("status", context).catch(() => {}),
    );
    for (const id of cases)
      ctx.commands.register(id, async (context) => {
        record(`pending ${id}`);
        await new Promise((resolve) => setTimeout(resolve, 4000));
        try {
          await ctx.composer.appendText(context, ` CI delayed ${id}.`);
          record(`appended ${id}`);
        } catch (error) {
          record(
            `cancelled ${id}: ${error instanceof PluginError ? error.code : "unknown"}`,
          );
        }
      });
  },
});
