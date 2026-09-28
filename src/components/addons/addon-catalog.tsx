import { useEffect, useState } from "react";
import { Search, RefreshCw, ShieldCheck } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ProblemAlert } from "@/components/settings/addon-parts";
import {
  TIER_LABELS,
  addonProblem,
  platformLabel,
  type AddonProblem,
} from "@/components/settings/addon-presentation";
import { useAddonsStore } from "@/stores/addons-store";
import { addonInvoke } from "@/lib/addons/bridge";
import {
  addonMessage,
  type AddonCatalogBrowse,
  type AddonReview,
} from "@/lib/addons/types";
export function AddonCatalog({
  onReview,
  busy,
  perform,
}: {
  onReview: (review: AddonReview) => void;
  busy: boolean;
  perform: (action: () => Promise<unknown>) => Promise<void>;
}) {
  const installed = useAddonsStore((s) => s.installed);
  const [catalog, setCatalog] = useState<AddonCatalogBrowse | null>(null);
  const [query, setQuery] = useState("");
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const [versions, setVersions] = useState<Record<string, string>>({});
  const [problems, setProblems] = useState<Record<string, AddonProblem>>({});
  const load = async (refresh: boolean) => {
    setLoading(true);
    setFailure("");
    try {
      setCatalog(
        await addonInvoke<AddonCatalogBrowse>("addon_catalog", { refresh }),
      );
    } catch (e) {
      setFailure(addonMessage(e));
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    void load(false);
  }, []);
  const plugins =
    catalog?.snapshot?.catalog.plugins.filter((p) =>
      `${p.name} ${p.id} ${p.description} ${p.publisher}`
        .toLowerCase()
        .includes(query.toLowerCase()),
    ) ?? [];
  return (
    <section className="space-y-4" aria-label="Browse add-ons">
      <div className="flex flex-wrap gap-2">
        <div className="relative min-w-40 flex-1">
          <Search className="absolute left-3 top-2.5 size-4 text-muted-foreground" />
          <Input
            aria-label="Search add-ons"
            className="pl-9"
            placeholder="Search by name, author, or task"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <Button
          variant="outline"
          disabled={busy || loading}
          onClick={() => void load(true)}
        >
          <RefreshCw className="size-4" />
          Refresh
        </Button>
      </div>
      {catalog?.snapshot && (
        <p className="text-label text-muted-foreground">
          {catalog.stale ? "Cached catalog" : "Catalog"} · revision{" "}
          {catalog.snapshot.catalog.revision} · checked{" "}
          {new Date(catalog.snapshot.fetchedAt * 1000).toLocaleString()}
        </p>
      )}
      {(failure || catalog?.error) && (
        <p role="status" className="rounded-lg border p-3 text-body">
          {failure || catalog?.error} Installed packages remain available
          offline. New installations require a successful refresh.
        </p>
      )}
      {loading ? (
        <p role="status" className="text-body text-muted-foreground">
          Checking the reviewed catalog…
        </p>
      ) : !plugins.length ? (
        <p className="py-8 text-center text-body text-muted-foreground">
          {query
            ? "No add-ons match your search."
            : "No reviewed releases are available yet."}
        </p>
      ) : (
        <div className="divide-y rounded-lg border">
          {plugins.map((plugin) => {
            // Only an installation from this listing's own source is "this" add-on.
            const current = installed.find(
              (i) =>
                i.manifest.id === plugin.id &&
                i.source.kind === "catalog" &&
                i.source.publisher === plugin.publisher &&
                i.source.repository === plugin.repository,
            );
            return (
              <article key={plugin.id} className="space-y-3 p-4">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <h3 className="font-medium">{plugin.name}</h3>
                    <p className="text-label text-muted-foreground">
                      {plugin.publisher} · {TIER_LABELS[plugin.tier]}
                    </p>
                  </div>
                  <div className="flex flex-wrap items-center justify-end gap-2">
                    {current && (
                      <Badge variant="secondary">
                        Installed · {current.manifest.version}
                      </Badge>
                    )}
                    {current?.updateAvailable && (
                      <Badge variant="outline">
                        Update {current.updateAvailable} available
                      </Badge>
                    )}
                    {plugin.tier === "official" && (
                      <ShieldCheck
                        className="size-4"
                        role="img"
                        aria-label="Official"
                      />
                    )}
                  </div>
                </div>
                <p className="text-body text-muted-foreground">
                  {plugin.description}
                </p>
                <div className="flex flex-wrap items-center gap-2">
                  <select
                    aria-label={`Release of ${plugin.name}`}
                    className="rounded-sm border bg-background px-2 py-1 text-body"
                    value={versions[plugin.id] ?? ""}
                    onChange={(e) =>
                      setVersions({ ...versions, [plugin.id]: e.target.value })
                    }
                  >
                    <option value="">Latest compatible release</option>
                    {plugin.releases.map((release) => (
                      <option key={release.version} value={release.version}>
                        {release.version} · API {release.api} ·{" "}
                        {release.platforms.map(platformLabel).join(", ")}
                      </option>
                    ))}
                  </select>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={() =>
                      void perform(async () => {
                        const target = versions[plugin.id]
                          ? `https://codemux.org/addons/${plugin.id}?version=${encodeURIComponent(versions[plugin.id])}`
                          : plugin.id;
                        setProblems((all) => {
                          const next = { ...all };
                          delete next[plugin.id];
                          return next;
                        });
                        try {
                          onReview(
                            await addonInvoke<AddonReview>(
                              "addon_catalog_review",
                              { target },
                            ),
                          );
                        } catch (e) {
                          // Explain it on this listing, where the user acted.
                          setProblems((all) => ({
                            ...all,
                            [plugin.id]: addonProblem(e),
                          }));
                        }
                      })
                    }
                  >
                    Review installation
                  </Button>
                </div>
                <ProblemAlert problem={problems[plugin.id] ?? null} />
              </article>
            );
          })}
        </div>
      )}
      <p className="text-label text-muted-foreground">
        Listings are reviewed contributions, not a guarantee against defects.
        While add-ons are installed, CodeMux rechecks the catalog about once a
        day for revocations and updates. Offline devices learn new revocations
        when they reconnect.
      </p>
    </section>
  );
}
