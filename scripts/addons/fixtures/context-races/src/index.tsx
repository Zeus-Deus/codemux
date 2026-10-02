import {
  definePlugin,
  Heading,
  type Handler,
  PluginError,
  Stack,
  Text,
  useEffect,
  useReducer,
} from "@codemux/plugin-sdk";

// CI-only: the harness releases each append after changing its context, so
// slow native UI operations cannot lose a race against a fixed timer. The
// disable case keeps a timer because disabling stops the fixture itself.
const cases = ["typing", "workspace", "thread", "replacement", "disable"];
const outcomes: string[] = [];
const listeners = new Set<() => void>();
const pending = new Map<string, () => void>();
function record(line: string) {
  console.info(`CI ${line}`);
  outcomes.push(line);
  if (outcomes.length > 12) outcomes.shift();
  for (const listener of listeners) listener();
}
function Status() {
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    const update = () => rerender(undefined);
    listeners.add(update);
    return () => {
      listeners.delete(update);
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
    ctx.commands.register("release", () => {
      for (const [id, release] of pending) {
        pending.delete(id);
        release();
      }
    });
    for (const id of cases) {
      const handler: Handler = async (context) => {
        const ready = id === "disable"
          ? new Promise<void>((resolve) => setTimeout(resolve, 4000))
          : new Promise<void>((resolve) => pending.set(id, resolve));
        record(`pending ${id}`);
        await ready;
        try {
          await ctx.composer.appendText(context, ` CI delayed ${id}.`);
          record(`appended ${id}`);
        } catch (error) {
          record(
            `cancelled ${id}: ${error instanceof PluginError ? error.code : "unknown"}`,
          );
        }
      };
      ctx.commands.register(id, handler);
      ctx.composerActions.register(id, handler);
    }
  },
});
