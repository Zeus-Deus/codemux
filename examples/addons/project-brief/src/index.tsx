import {
  definePlugin,
  Stack,
  Text,
  Heading,
  Button,
  List,
  Checkbox,
  useState,
  useEffect,
  PluginError,
  type PluginContext,
  type ContextHandle,
  type ViewProps,
  type Workspace,
  type GitSummary,
} from "@codemux/plugin-sdk";
const message = (error: unknown) =>
  error instanceof Error ? error.message : "Project information is unavailable";
export default definePlugin({
  activate(ctx) {
    async function load(context: ContextHandle) {
      const workspace = await ctx.workspace.current(context);
      if (!workspace)
        throw new Error("Open a local project to prepare a brief.");
      let git: GitSummary | null = null;
      try {
        git = await ctx.git.summary(context);
      } catch (error) {
        if (!(error instanceof PluginError) || error.code !== "NOT_A_GIT_REPO")
          throw error;
      }
      return { workspace, git };
    }
    async function insert(
      context: ContextHandle,
      data?: { workspace: Workspace; git: GitSummary | null },
    ) {
      const current = data ?? (await load(context));
      const settings = await ctx.settings.get();
      const { workspace, git } = current;
      const lines = [
        `Project: ${workspace.name}`,
        git
          ? `Branch: ${git.branch ?? "detached HEAD"}`
          : "Not a Git repository",
      ];
      if (git) {
        lines.push(
          `Changes: ${git.staged} staged, ${git.unstaged} unstaged, ${git.untracked} untracked, ${git.conflicts} conflicts`,
        );
        if (settings["include-files"] === true && git.paths.length)
          lines.push(
            `Changed paths: ${git.paths.slice(0, 20).join(", ")}${git.paths.length > 20 || git.truncated ? " …" : ""}`,
          );
      }
      await ctx.composer.appendText(context, lines.join("\n"));
    }
    function Brief({ context }: ViewProps) {
      const [data, setData] = useState<{
        workspace: Workspace;
        git: GitSummary | null;
      } | null>(null);
      const [error, setError] = useState("");
      const [loading, setLoading] = useState(true);
      const [expanded, setExpanded] = useState(true);
      useEffect(() => {
        let live = true;
        load(context)
          .then((value) => {
            if (live) setData(value);
          })
          .catch((e) => {
            if (live) setError(message(e));
          })
          .finally(() => {
            if (live) setLoading(false);
          });
        ctx.storage
          .get({ scope: "global" }, "show-paths")
          .then((value) => {
            if (live && typeof value === "boolean") setExpanded(value);
          })
          .catch(() => {});
        return () => {
          live = false;
        };
      }, [context]);
      return (
        <Stack spacing="md">
          <Heading>Project Brief</Heading>
          <Text color="muted">Local context, ready for your draft.</Text>
          {loading ? (
            <Text>Reading project…</Text>
          ) : data ? (
            <Stack spacing="sm">
              <Heading level={3}>{data.workspace.name}</Heading>
              <Text>
                {data.git
                  ? `Branch: ${data.git.branch ?? "detached HEAD"}`
                  : "Not a Git repository"}
              </Text>
              {data.git && (
                <>
                  <Text>{`${data.git.staged} staged · ${data.git.unstaged} unstaged · ${data.git.untracked} untracked · ${data.git.conflicts} conflicts`}</Text>
                  <Checkbox
                    label="Show changed paths"
                    checked={expanded}
                    onChange={async (event) => {
                      setExpanded(event.value === true);
                      await ctx.storage.set(
                        { scope: "global" },
                        "show-paths",
                        event.value === true,
                      );
                    }}
                  />
                  {expanded &&
                    (data.git.paths.length ? (
                      <List items={data.git.paths} />
                    ) : (
                      <Text color="muted">Working tree is clean.</Text>
                    ))}
                  {data.git.truncated && (
                    <Text color="muted">
                      Showing the first 500 changed paths.
                    </Text>
                  )}
                </>
              )}
            </Stack>
          ) : null}
          {error && <Text color="danger">{error}</Text>}
          <Stack direction="horizontal">
            <Button
              disabled={loading}
              onPress={async (event) => {
                setLoading(true);
                setError("");
                try {
                  setData(await load(event.context));
                  await ctx.storage.set(
                    { scope: "workspace", context: event.context },
                    "last-refreshed",
                    Date.now(),
                  );
                } catch (e) {
                  setError(message(e));
                } finally {
                  setLoading(false);
                }
              }}
            >
              Refresh
            </Button>
            <Button
              disabled={!data || loading}
              onPress={async (event) => {
                try {
                  await insert(event.context, data!);
                  setError("");
                } catch (e) {
                  setError(message(e));
                }
              }}
            >
              Add to draft
            </Button>
          </Stack>
        </Stack>
      );
    }
    ctx.panels.register("brief", (props) => <Brief {...props} />);
    ctx.commands.register("open", (context) =>
      ctx.panels.open("brief", context),
    );
    ctx.composerActions.register("insert", async (context) => {
      try {
        await insert(context);
      } catch (error) {
        await ctx.ui.notify(message(error));
      }
    });
  },
});
