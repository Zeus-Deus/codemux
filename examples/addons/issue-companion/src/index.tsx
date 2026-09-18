import {
  definePlugin,
  Stack,
  Text,
  Heading,
  Button,
  Select,
  useEffect,
  useState,
  type ViewProps,
  type ContextHandle,
} from "@codemux/plugin-sdk";
type Issue = { number: number; title: string; html_url: string };
export default definePlugin({
  activate(ctx) {
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
      const response = await ctx.http.fetch(context, {
        origin: "https://api.github.com",
        path: `/repos/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/issues?state=open&per_page=50`,
        method: "GET",
        headers: { Accept: "application/vnd.github+json" },
      });
      if (response.status === 401)
        throw new Error(
          "GitHub did not accept the configured credential. Update it in Add-ons settings.",
        );
      if (
        response.status === 429 ||
        (response.status === 403 &&
          response.headers["x-ratelimit-remaining"] === "0")
      )
        throw new Error(
          "GitHub rate limit reached. Wait before refreshing or configure a token.",
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
        .slice(0, 50);
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
            if (live) setError(e.message);
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
    ctx.commands.register("open", (context) =>
      ctx.panels.open("issues", context),
    );
    ctx.panels.register("issues", (props) => <Issues {...props} />);
    ctx.composerActions.register("browse", (context) =>
      ctx.composerViews.open("issues", context),
    );
    ctx.composerViews.register("issues", (props) => <Issues {...props} />);
  },
});
