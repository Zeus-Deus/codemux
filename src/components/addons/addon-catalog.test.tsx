import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
vi.mock("@/lib/addons/bridge", () => ({
  addonInvoke: vi.fn().mockResolvedValue(null),
}));
import { AddonCatalog } from "./addon-catalog";
import { useAddonsStore } from "@/stores/addons-store";
import { addonInvoke } from "@/lib/addons/bridge";
import type {
  AddonCatalogBrowse,
  AddonCatalogPlugin,
  AddonInstallation,
  AddonManifest,
} from "@/lib/addons/types";
import manifest from "../../../examples/addons/issue-companion/manifest.json";

const plugin = (fields: Partial<AddonCatalogPlugin> = {}): AddonCatalogPlugin => ({
  id: "codemux.issue-companion",
  name: "Issue Companion",
  publisher: "Reviewed Publisher",
  tier: "community",
  description: "Browse issues.",
  repository: "https://github.com/example/issue-companion",
  readme: "",
  releases: [
    {
      version: "1.1.0",
      api: "^1.0.0",
      platforms: ["linux-x64"],
      sha256: "c".repeat(64),
      publishedAt: "2026-09-01",
      capabilities: { permissions: [], http: [], credentials: [] },
    },
  ],
  ...fields,
});
const browse = (fields: Partial<AddonCatalogBrowse> = {}): AddonCatalogBrowse => ({
  snapshot: {
    fetchedAt: Date.UTC(2026, 8, 20, 12) / 1000,
    catalog: { revision: 7, plugins: [plugin()] },
  },
  error: null,
  stale: false,
  ...fields,
});
const installed: AddonInstallation = {
  installationId: "catalog-fixture",
  manifest: manifest as AddonManifest,
  source: {
    kind: "catalog",
    publisher: "Reviewed Publisher",
    repository: "https://github.com/example/issue-companion",
  },
  digest: "fixture-digest",
  dataGeneration: "fixture-data",
  desiredEnabled: true,
  status: "enabled-idle",
  failure: null,
  previous: null,
  updateAvailable: "1.1.0",
};
// Runs the action like Settings does, surfacing anything it rethrows.
const perform = vi.fn(async (action: () => Promise<unknown>) => {
  await action();
});
beforeEach(() => {
  perform.mockClear();
  useAddonsStore.setState({ installed: [] });
});
afterEach(cleanup);

it("labels a cached catalog with its freshness and keeps installed add-ons usable offline", async () => {
  vi.mocked(addonInvoke)
    .mockReset()
    .mockResolvedValue(browse({ stale: true, error: "Add-on download failed" }));
  render(<AddonCatalog onReview={vi.fn()} busy={false} perform={perform} />);
  expect(
    await screen.findByText(/^Cached catalog · revision 7 · checked /),
  ).toBeInTheDocument();
  expect(
    screen.getByText(
      /Add-on download failed Installed packages remain available offline\. New installations require a successful refresh\./,
    ),
  ).toBeInTheDocument();
  expect(addonInvoke).toHaveBeenCalledWith("addon_catalog", { refresh: false });
});

it("marks an installed listing and its available update", async () => {
  vi.mocked(addonInvoke).mockReset().mockResolvedValue(browse());
  useAddonsStore.setState({ installed: [installed] });
  render(<AddonCatalog onReview={vi.fn()} busy={false} perform={perform} />);
  const listing = within(
    (await screen.findByRole("heading", { name: "Issue Companion" })).closest(
      "article",
    )!,
  );
  expect(listing.getByText("Installed · 1.0.0")).toBeInTheDocument();
  expect(listing.getByText("Update 1.1.0 available")).toBeInTheDocument();
  expect(listing.getByText("Reviewed Publisher · Community")).toBeInTheDocument();
});

it("does not treat a same-ID installation from another source as this listing", async () => {
  vi.mocked(addonInvoke).mockReset().mockResolvedValue(browse());
  useAddonsStore.setState({
    installed: [{ ...installed, source: { kind: "local", identity: "fixture" } }],
  });
  render(<AddonCatalog onReview={vi.fn()} busy={false} perform={perform} />);
  await screen.findByRole("heading", { name: "Issue Companion" });
  expect(screen.queryByText(/Installed ·/)).toBeNull();
});

it("explains a rejected review on the listing the user chose", async () => {
  vi.mocked(addonInvoke)
    .mockReset()
    .mockImplementation((command) =>
      command === "addon_catalog"
        ? Promise.resolve(browse())
        : Promise.reject({
            message:
              "Version 1.1.0 of codemux.issue-companion requires add-on API ^2.0.0; this CodeMux provides 1.0.0",
            data: { code: "INCOMPATIBLE_API" },
          }),
    );
  const onReview = vi.fn();
  render(<AddonCatalog onReview={onReview} busy={false} perform={perform} />);
  const heading = await screen.findByRole("heading", { name: "Issue Companion" });
  const listing = within(heading.closest("article")!);
  fireEvent.change(listing.getByRole("combobox"), {
    target: { value: "1.1.0" },
  });
  fireEvent.click(listing.getByRole("button", { name: "Review installation" }));
  expect(await listing.findByRole("alert")).toHaveTextContent(
    /^Incompatible add-on API\. Version 1\.1\.0 of codemux\.issue-companion requires/,
  );
  expect(addonInvoke).toHaveBeenCalledWith("addon_catalog_review", {
    target: "https://codemux.org/addons/codemux.issue-companion?version=1.1.0",
  });
  expect(onReview).not.toHaveBeenCalled();
  await waitFor(() => expect(perform).toHaveBeenCalledTimes(1));
});
