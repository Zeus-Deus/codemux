/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type { ProviderCapabilities, ProviderDiagnostic } from "@/tauri/types";

vi.mock("@/tauri/commands", async (importActual) => {
  const actual = (await importActual()) as Record<string, unknown>;
  return { ...actual, discoverSourceControl: vi.fn() };
});

import { SourceControlSection, normalizeHostInput } from "./source-control-section";
import { discoverSourceControl } from "@/tauri/commands";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";

const discoverMock = discoverSourceControl as unknown as ReturnType<typeof vi.fn>;

const NO_CAPABILITIES: ProviderCapabilities = {
  has_pull_requests: false,
  has_checks: false,
  has_issues: false,
  has_inline_comments: false,
  has_review_threads: false,
  has_deployments: false,
  has_reviews: false,
  has_fork_pr_fetch: false,
};

function diag(over: Partial<ProviderDiagnostic> & { kind: string }): ProviderDiagnostic {
  return {
    supported: true,
    cli_installed: true,
    cli_version: null,
    authenticated: true,
    account: null,
    detail: null,
    capabilities: NO_CAPABILITIES,
    ...over,
  };
}

const READY_GITHUB = diag({
  kind: "github",
  cli_version: "gh version 2.63.2",
  account: "octo-dev",
});
const MISSING_GITLAB = diag({
  kind: "gitlab",
  cli_installed: false,
  authenticated: false,
  detail: "`glab` was not found on PATH, so Codemux cannot talk to GitLab.",
});
const UNSUPPORTED = [
  diag({ kind: "bitbucket", supported: false, cli_installed: false, authenticated: false }),
  diag({ kind: "azure_devops", supported: false, cli_installed: false, authenticated: false }),
];

let updateSettingSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  discoverMock.mockReset();
  discoverMock.mockResolvedValue([READY_GITHUB, MISSING_GITLAB, ...UNSUPPORTED]);
  updateSettingSpy = vi.fn().mockResolvedValue(undefined);
  useSyncedSettingsStore.setState({
    settings: {
      ...useSyncedSettingsStore.getState().settings,
      source_control: { custom_hosts: {} },
    },
    updateSetting: updateSettingSpy,
  } as never);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("provider diagnostics", () => {
  it("marks an installed + signed-in product ready and shows its version", async () => {
    render(<SourceControlSection />);
    const row = await screen.findByTestId("source-control-row-github");
    expect(row).toHaveAttribute("data-status", "ready");
    expect(row).toHaveTextContent("GitHub");
    expect(row).toHaveTextContent("Ready");
    expect(row).toHaveTextContent("gh version 2.63.2");
  });

  it("distinguishes a missing CLI from a signed-out one, with the matching fix", async () => {
    // The two states have different remedies; conflating them is the
    // exact confusion this pane exists to remove.
    discoverMock.mockResolvedValue([
      READY_GITHUB,
      diag({ kind: "gitlab", cli_installed: false, authenticated: false }),
      ...UNSUPPORTED,
    ]);
    const { unmount } = render(<SourceControlSection />);
    let row = await screen.findByTestId("source-control-row-gitlab");
    expect(row).toHaveAttribute("data-status", "attention");
    expect(row).toHaveTextContent("CLI not installed");
    expect(row).toHaveTextContent("gitlab.com/gitlab-org/cli");
    expect(row).not.toHaveTextContent("glab auth login");
    unmount();
    cleanup();

    discoverMock.mockResolvedValue([
      READY_GITHUB,
      diag({ kind: "gitlab", cli_version: "glab 1.36.0", authenticated: false }),
      ...UNSUPPORTED,
    ]);
    render(<SourceControlSection />);
    row = await screen.findByTestId("source-control-row-gitlab");
    expect(row).toHaveAttribute("data-status", "attention");
    expect(row).toHaveTextContent("Not signed in");
    expect(row).toHaveTextContent("glab auth login");
  });

  it("renders products with no adapter as inert rows", async () => {
    render(<SourceControlSection />);
    for (const kind of ["bitbucket", "azure_devops"]) {
      const row = await screen.findByTestId(`source-control-row-${kind}`);
      expect(row).toHaveAttribute("data-status", "inert");
      expect(row).toHaveTextContent("Not yet supported");
    }
    expect(
      await screen.findByTestId("source-control-row-azure_devops"),
    ).toHaveTextContent("Azure DevOps");
  });

  it("masks the account until it is clicked", async () => {
    render(<SourceControlSection />);
    await screen.findByTestId("source-control-row-github");
    const reveal = screen.getByRole("button", { name: /reveal account name/i });
    expect(reveal).not.toHaveTextContent("octo-dev");
    fireEvent.click(reveal);
    expect(
      screen.getByRole("button", { name: /hide account name/i }),
    ).toHaveTextContent("octo-dev");
  });

  it("re-probes on Rescan", async () => {
    render(<SourceControlSection />);
    await screen.findByTestId("source-control-row-github");
    expect(discoverMock).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: /rescan/i }));
    await waitFor(() => expect(discoverMock).toHaveBeenCalledTimes(2));
  });

  it("still lists every product when the backend returns a short list", async () => {
    // A partial answer must not silently drop a product — the user
    // would read the absence as "Codemux doesn't support GitLab".
    discoverMock.mockResolvedValue([READY_GITHUB]);
    render(<SourceControlSection />);
    for (const kind of ["github", "gitlab", "bitbucket", "azure_devops"]) {
      expect(
        await screen.findByTestId(`source-control-row-${kind}`),
      ).toBeInTheDocument();
    }
  });

  it("surfaces a probe failure without blanking the pane", async () => {
    discoverMock.mockRejectedValue("backend exploded");
    render(<SourceControlSection />);
    expect(await screen.findByTestId("source-control-error")).toHaveTextContent(
      "backend exploded",
    );
    expect(
      await screen.findByTestId("source-control-row-github"),
    ).toBeInTheDocument();
  });
});

describe("custom hosts editor", () => {
  it("lists a configured self-hosted mapping", async () => {
    useSyncedSettingsStore.setState({
      settings: {
        ...useSyncedSettingsStore.getState().settings,
        source_control: { custom_hosts: { "git.acme.internal": "gitlab" } },
      },
    } as never);
    render(<SourceControlSection />);
    expect(
      await screen.findByTestId("custom-host-git.acme.internal"),
    ).toHaveTextContent("git.acme.internal");
  });

  it("writes a new mapping through the synced-settings store", async () => {
    render(<SourceControlSection />);
    fireEvent.click(await screen.findByRole("button", { name: /add self-hosted server/i }));
    fireEvent.change(screen.getByLabelText(/hostname/i), {
      target: { value: "git.acme.internal" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^add$/i }));
    await waitFor(() =>
      expect(updateSettingSpy).toHaveBeenCalledWith("source_control", "custom_hosts", {
        "git.acme.internal": "gitlab",
      }),
    );
  });

  it("rejects empty input instead of writing a blank key", async () => {
    render(<SourceControlSection />);
    fireEvent.click(await screen.findByRole("button", { name: /add self-hosted server/i }));
    fireEvent.click(screen.getByRole("button", { name: /^add$/i }));
    expect(screen.getByText(/enter a hostname/i)).toBeInTheDocument();
    expect(updateSettingSpy).not.toHaveBeenCalled();
  });

  it("removes a mapping", async () => {
    useSyncedSettingsStore.setState({
      settings: {
        ...useSyncedSettingsStore.getState().settings,
        source_control: {
          custom_hosts: { "git.acme.internal": "gitlab", "gh.acme.internal": "github" },
        },
      },
    } as never);
    render(<SourceControlSection />);
    fireEvent.click(
      await screen.findByRole("button", { name: /remove git\.acme\.internal/i }),
    );
    await waitFor(() =>
      expect(updateSettingSpy).toHaveBeenCalledWith("source_control", "custom_hosts", {
        "gh.acme.internal": "github",
      }),
    );
  });

  it("tolerates settings written without a source_control section", async () => {
    // Older synced blobs predate the section entirely; reading them must
    // not throw on the way to an empty editor.
    useSyncedSettingsStore.setState({
      settings: {
        ...useSyncedSettingsStore.getState().settings,
        source_control: undefined,
      },
    } as never);
    render(<SourceControlSection />);
    expect(
      await screen.findByRole("button", { name: /add self-hosted server/i }),
    ).toBeInTheDocument();
  });
});

describe("normalizeHostInput", () => {
  it("reduces the shapes people actually paste to a bare hostname", () => {
    for (const input of [
      "git.acme.internal",
      "  GIT.Acme.Internal  ",
      "https://git.acme.internal",
      "https://git.acme.internal/",
      "https://git.acme.internal/group/repo.git",
      "git@git.acme.internal:group/repo.git",
      "ssh://git@git.acme.internal:2222/group/repo.git",
      "https://git.acme.internal:8443/group/repo",
    ]) {
      expect(normalizeHostInput(input)).toBe("git.acme.internal");
    }
  });

  it("rejects input that is not a hostname", () => {
    for (const bad of ["", "   ", "not a host", "*", "http://"]) {
      expect(normalizeHostInput(bad)).toBeNull();
    }
  });
});
