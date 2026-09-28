import {
  definePlugin,
  Stack,
  Text,
  Heading,
  Button,
  Select,
  useEffect,
  useState,
  PluginError,
  type ViewProps,
  type ContextHandle,
} from "@codemux/plugin-sdk";
type Issue = { number: number; title: string; html_url: string };
const describe = (error: unknown) =>
  error instanceof Error ? error.message : "Issue Companion is unavailable";
// GitHub reports when a limit resets in seconds, either relative or absolute.
function retryHint(headers: Record<string, string>) {
  const retry = Number(headers["retry-after"]) * 1000;
  const reset = Number(headers["x-ratelimit-reset"]) * 1000 - Date.now();
  const wait = retry > 0 ? retry : reset > 0 ? reset : 0;
  if (!wait) return "Wait before refreshing";
  const minutes = Math.ceil(wait / 60000);
  return `Try again in about ${minutes} minute${minutes === 1 ? "" : "s"}`;
}
export default definePlugin({
  activate(ctx) {
    // Notifications are limited to three per minute; a rejected one is dropped.
    const notify = (text: string) => ctx.ui.notify(text).catch(() => {});
    async function fetchIssues(context: ContextHandle): Promise<Issue[]> {
      const settings = await ctx.settings.get();
      const owner = settings.owner,
        repo = settings.repository;
      if (
        typeof owner !== "string" ||
        typeof repo !== "string" ||
        !/^[A-Za-z0-9-]+$/.test(owner) ||
        !/^[A-Za-z0-9_.-]+$/.test(repo)
      )
        throw new Error(
          "Configure a repository owner and name in Add-ons settings.",
        );
      let response;
      try {
        // Twenty issues keep typical responses well below the 512 KiB limit.
        response = await ctx.http.fetch(context, {
          origin: "https://api.github.com",
          path: `/repos/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/issues?state=open&per_page=20`,
          method: "GET",
          headers: { Accept: "application/vnd.github+json" },
        });
      } catch (error) {
        if (
          error instanceof PluginError &&
          error.code === "CREDENTIAL_REQUIRED"
        )
          throw new Error(
            "The saved GitHub token cannot be read. Unlock the system keyring or enter the token again in Add-ons settings.",
          );
        if (error instanceof PluginError && error.code === "RESOURCE_LIMIT")
          throw new Error(
            `Issues could not be loaded within add-on limits (${error.message}). Try again shortly.`,
          );
        throw error;
      }
      if (response.status === 401)
        throw new Error(
          "GitHub did not accept the configured credential. Update it in Add-ons settings.",
        );
      // GitHub signals primary and secondary rate limits with 429, or with 403
      // plus an exhausted quota or a retry-after header.
      if (
        response.status === 429 ||
        (response.status === 403 &&
          (response.headers["x-ratelimit-remaining"] === "0" ||
            response.headers["retry-after"] !== undefined))
      )
        throw new Error(
          `GitHub rate limit reached. ${retryHint(response.headers)}, or configure a token for a higher limit.`,
        );
      if (response.status === 403)
        throw new Error(
          "GitHub denied access to this repository. Check that the configured token may read it, or remove the token to use public access.",
        );
      if (response.status === 404)
        throw new Error(
          "Repository not found or the credential cannot access it.",
        );
      if (response.status !== 200)
        throw new Error(
          `GitHub returned status ${response.status}. Try again later.`,
        );
      const data: unknown = JSON.parse(response.body);
      if (!Array.isArray(data))
        throw new Error("GitHub returned an unexpected response.");
      return data
        .filter(
          (item): item is Issue =>
            item &&
            typeof item === "object" &&
            !item.pull_request &&
            Number.isSafeInteger(item.number) &&
            typeof item.title === "string" &&
            item.title.length <= 1000 &&
            typeof item.html_url === "string" &&
            item.html_url.startsWith("https://github.com/"),
        )
        .slice(0, 20);
    }
    function Issues({ context }: ViewProps) {
      const [issues, setIssues] = useState<Issue[]>([]),
        [selected, setSelected] = useState(""),
        [loading, setLoading] = useState(true),
        [error, setError] = useState("");
      useEffect(() => {
        let live = true;
        fetchIssues(context)
          .then((data) => {
            if (live) {
              setIssues(data);
              setSelected(String(data[0]?.number ?? ""));
            }
          })
          .catch((e) => {
            if (live) setError(describe(e));
          })
          .finally(() => {
            if (live) setLoading(false);
          });
        return () => {
          live = false;
        };
      }, [context]);
      const issue = issues.find((i) => String(i.number) === selected);
      return (
        <Stack spacing="md">
          <Heading>Issue Companion</Heading>
          <Text color="muted">
            Browse issues from your configured GitHub repository.
          </Text>
          {loading ? (
            <Text>Loading issues…</Text>
          ) : issues.length ? (
            <Select
              label="Open issue"
              value={selected}
              options={issues.map((i) => ({
                label: `#${i.number} ${i.title}`,
                value: String(i.number),
              }))}
              onChange={(event) => setSelected(String(event.value))}
            />
          ) : !error ? (
            <Text>No open issues.</Text>
          ) : null}
          {issue && <Text>{`#${issue.number} ${issue.title}`}</Text>}
          {error && <Text color="danger">{error}</Text>}
          <Stack direction="horizontal">
            <Button
              disabled={loading}
              onPress={async (event) => {
                setLoading(true);
                setError("");
                try {
                  const data = await fetchIssues(event.context);
                  setIssues(data);
                  setSelected(String(data[0]?.number ?? ""));
                } catch (e) {
                  setError(
                    e instanceof Error ? e.message : "Could not load issues",
                  );
                } finally {
                  setLoading(false);
                }
              }}
            >
              Refresh
            </Button>
            <Button
              disabled={!issue}
              onPress={async (event) => {
                try {
                  await ctx.links.open(issue!.html_url, event.context);
                } catch (e) {
                  setError(e instanceof Error ? e.message : "Link unavailable");
                }
              }}
            >
              Open in browser
            </Button>
            <Button
              disabled={!issue}
              onPress={async (event) => {
                try {
                  await ctx.composer.appendText(
                    event.context,
                    `${issue!.title}\n${issue!.html_url}`,
                  );
                  setError("");
                } catch (e) {
                  setError(
                    e instanceof Error ? e.message : "Draft unavailable",
                  );
                }
              }}
            >
              Add to draft
            </Button>
          </Stack>
        </Stack>
      );
    }
    ctx.commands.register("open", async (context) => {
      try {
        await ctx.panels.open("issues", context);
      } catch (error) {
        await notify(describe(error));
      }
    });
    ctx.panels.register("issues", (props) => <Issues {...props} />);
    ctx.composerActions.register("browse", async (context) => {
      try {
        await ctx.composerViews.open("issues", context);
      } catch (error) {
        await notify(describe(error));
      }
    });
    ctx.composerViews.register("issues", (props) => <Issues {...props} />);
  },
});
