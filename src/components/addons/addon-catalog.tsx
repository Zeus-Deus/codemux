import { useEffect, useState } from "react";
import { Search, RefreshCw, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { addonInvoke } from "@/lib/addons/bridge";
import {
  addonMessage,
  type AddonReview,
  type AddonManifest,
} from "@/lib/addons/types";
type Release = {
  version: string;
  api: string;
  platforms: string[];
  sha256: string;
  publishedAt: string;
  capabilities: Pick<AddonManifest, "permissions" | "http" | "credentials">;
};
type CatalogPlugin = {
  id: string;
  name: string;
  publisher: string;
  tier: "official" | "community";
  description: string;
  repository: string;
  readme: string;
  releases: Release[];
};
type Browse = {
  snapshot: {
    fetchedAt: number;
    catalog: { revision: number; plugins: CatalogPlugin[] };
  } | null;
  error: string | null;
  stale: boolean;
};
export function AddonCatalog({
  onReview,
  busy,
  perform,
}: {
  onReview: (review: AddonReview) => void;
  busy: boolean;
  perform: (action: () => Promise<unknown>) => Promise<void>;
}) {
  const [catalog, setCatalog] = useState<Browse | null>(null);
  const [query, setQuery] = useState("");
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(false);
  const [versions, setVersions] = useState<Record<string, string>>({});
  const load = async (refresh: boolean) => {
    setLoading(true);
    setFailure("");
    try {
      setCatalog(await addonInvoke<Browse>("addon_catalog", { refresh }));
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
          {plugins.map((plugin) => (
            <article key={plugin.id} className="space-y-3 p-4">
              <div className="flex items-start justify-between gap-3">
                <div>
                  <h3 className="font-medium">{plugin.name}</h3>
                  <p className="text-label text-muted-foreground">
                    {plugin.publisher} · {plugin.tier}
                  </p>
                </div>
                {plugin.tier === "official" && (
                  <ShieldCheck className="size-4" aria-label="Official" />
                )}
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
                      {release.platforms.join(", ")}
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
                      onReview(
                        await addonInvoke<AddonReview>("addon_catalog_review", {
                          target,
                        }),
                      );
                    })
                  }
                >
                  Review installation
                </Button>
              </div>
            </article>
          ))}
        </div>
      )}
      <p className="text-label text-muted-foreground">
        Listings are reviewed contributions, not a guarantee against defects.
        Offline devices learn new revocations when they reconnect.
      </p>
    </section>
  );
}
