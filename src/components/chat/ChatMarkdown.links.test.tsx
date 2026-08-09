/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
  toastError: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mocks.openUrl(...args),
}));

vi.mock("@/lib/toast", () => ({
  toast: { error: (...args: unknown[]) => mocks.toastError(...args) },
}));

import {
  docEditorTabId,
  docPaneId,
} from "@/components/layout/right-panel/pane-registry";
import { useEditorStore } from "@/stores/editor-store";
import { useUIStore } from "@/stores/ui-store";
import { TooltipProvider } from "@/components/ui/tooltip";

import { ChatMarkdown } from "./ChatMarkdown";
import { resetFaviconFailureCache } from "./MarkdownLinkFavicon";

beforeEach(() => {
  resetFaviconFailureCache();
  useEditorStore.setState({ tabs: {} });
  useUIStore.setState({ rightPanelTabs: {}, rightPanelPanes: {} });
  mocks.openUrl.mockReset().mockResolvedValue(undefined);
  mocks.toastError.mockReset();
});
afterEach(() => cleanup());

/** Middle click. `fireEvent` has no auxClick helper, and it never fires onClick. */
function middleClick(element: Element): boolean {
  return fireEvent(
    element,
    new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }),
  );
}

describe("ChatMarkdown rich external links", () => {
  it("adds the destination favicon to any labelled http(s) link", () => {
    const { container } = render(
      <ChatMarkdown>
        {"Committed and shipped: [PR #235](https://github.com/example/repo/pull/235)."}
      </ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    const favicon = link?.querySelector(".chat-markdown-link-favicon img");
    expect(link).toHaveTextContent("PR #235");
    expect(link?.querySelector(".chat-markdown-link-favicon")).toHaveAttribute(
      "title",
      "https://github.com/example/repo/pull/235",
    );
    expect(favicon).toHaveAttribute(
      "src",
      "https://www.google.com/s2/favicons?domain=github.com&sz=32",
    );
    expect(link?.querySelector(".chat-markdown-link-leading")).toHaveTextContent("P");
  });

  it("confirms external links in a body-level portal before opening the system browser", async () => {
    const { container } = render(
      <ChatMarkdown>
        {"[OpenAI documentation](https://platform.openai.com/docs)"}
      </ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    expect(link?.tagName).toBe("A");
    expect(link).toHaveAttribute("href", "https://platform.openai.com/docs");
    // The confirmed open goes through the Tauri opener, so a browsing context
    // target would only buy an unconfirmed middle-click navigation.
    expect(link).not.toHaveAttribute("target");

    fireEvent.click(link as HTMLAnchorElement);

    const dialog = screen.getByRole("alertdialog");
    expect(dialog.parentElement).toBe(document.body);
    expect(
      screen.getByText("https://platform.openai.com/docs"),
    ).toBeInTheDocument();
    expect(mocks.openUrl).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Open link" }));

    await waitFor(() =>
      expect(mocks.openUrl).toHaveBeenCalledWith(
        "https://platform.openai.com/docs",
      ),
    );
  });

  it("never navigates an external link itself, not even on middle click", () => {
    const { container } = render(
      <ChatMarkdown>{"[Reference](https://example.com/reference)"}</ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    // fireEvent returns false once a handler called preventDefault.
    expect(fireEvent.click(link as HTMLAnchorElement)).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(middleClick(link as HTMLAnchorElement)).toBe(false);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(mocks.openUrl).not.toHaveBeenCalled();
  });

  it("does not open an external link when its confirmation is cancelled", () => {
    const { container } = render(
      <ChatMarkdown>
        {"[Reference](https://example.com/reference)"}
      </ChatMarkdown>,
    );

    fireEvent.click(
      container.querySelector('[data-streamdown="link"]') as HTMLAnchorElement,
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(mocks.openUrl).not.toHaveBeenCalled();
  });

  it("surfaces an opener failure after confirmation", async () => {
    mocks.openUrl.mockRejectedValueOnce(new Error("opener unavailable"));
    const { container } = render(
      <ChatMarkdown>
        {"[Reference](https://example.com/reference)"}
      </ChatMarkdown>,
    );

    fireEvent.click(
      container.querySelector('[data-streamdown="link"]') as HTMLAnchorElement,
    );
    fireEvent.click(screen.getByRole("button", { name: "Open link" }));

    await waitFor(() =>
      expect(mocks.toastError).toHaveBeenCalledWith("Could not open the link", {
        description: "opener unavailable",
      }),
    );
  });

  it("keeps a bare URL's protocol with the favicon and leaves local links plain", () => {
    const { container } = render(
      <ChatMarkdown>
        {"https://docs.codemux.org and [local](#code-blocks) and [mail](mailto:hello@codemux.org)"}
      </ChatMarkdown>,
    );

    const links = container.querySelectorAll('[data-streamdown="link"]');
    expect(links).toHaveLength(3);
    expect(links[0]?.querySelector(".chat-markdown-link-leading")).toHaveTextContent(
      "https://",
    );
    expect(links[0]?.querySelector("img")).toHaveAttribute(
      "src",
      "https://www.google.com/s2/favicons?domain=docs.codemux.org&sz=32",
    );
    expect(links[1]?.querySelector(".chat-markdown-link-favicon")).toBeNull();
    expect(links[2]?.querySelector(".chat-markdown-link-favicon")).toBeNull();
  });

  it("falls back to a globe when a site's favicon fails", () => {
    const { container } = render(
      <ChatMarkdown>{"[Reference](https://missing-favicon.invalid/page)"}</ChatMarkdown>,
    );
    const wrapper = container.querySelector(".chat-markdown-link-favicon");
    const image = wrapper?.querySelector("img");
    expect(image).not.toBeNull();

    fireEvent.error(image as HTMLImageElement);

    expect(wrapper?.querySelector("img")).toBeNull();
    expect(wrapper?.querySelector("svg")).toBeInTheDocument();
  });

  it("holds only the label's first character in the nowrap span for styled labels", () => {
    const { container } = render(
      <ChatMarkdown>
        {"[**A very long bolded release title**](https://example.com/releases/1)"}
      </ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    const leading = link?.querySelector(".chat-markdown-link-leading");
    // Bold styling survives, and the icon sits inside it next to the first
    // character only — the rest of the label stays free to wrap.
    const strong = link?.querySelector('[data-streamdown="strong"]');
    expect(strong).toHaveTextContent("A very long bolded release title");
    // The nowrap span lives *inside* the styled label, not around it.
    expect(strong?.contains(leading as Node)).toBe(true);
    expect(leading?.textContent).toBe("A");
    expect(leading?.querySelector("img")).toHaveAttribute(
      "src",
      "https://www.google.com/s2/favicons?domain=example.com&sz=32",
    );
    expect(link).toHaveTextContent("A very long bolded release title");
  });

  it("keeps a leading emoji whole in the nowrap span", () => {
    const { container } = render(
      <ChatMarkdown>{"[🚀 Launch notes](https://example.com/launch)"}</ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    const leading = link?.querySelector(".chat-markdown-link-leading");
    // The whole code point moves, so no lone surrogate renders as a tofu box.
    expect(leading?.textContent).toBe("🚀");
    expect(link).toHaveTextContent("🚀 Launch notes");
  });

  it("keeps the favicon outside an inline-code label's pill", () => {
    const { container } = render(
      <ChatMarkdown>{"[`rich-links.ts`](https://example.com/src)"}</ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    const code = link?.querySelector("code");
    expect(code).toHaveTextContent("rich-links.ts");
    expect(code?.querySelector(".chat-markdown-link-favicon")).toBeNull();
    expect(link?.querySelector(".chat-markdown-link-favicon")).not.toBeNull();
  });

  it("requests no favicon for a non-public destination", () => {
    const { container } = render(
      <ChatMarkdown>{"[dashboard](http://192.168.1.10:8080/status)"}</ChatMarkdown>,
    );

    const wrapper = container.querySelector(".chat-markdown-link-favicon");
    expect(wrapper?.querySelector("img")).toBeNull();
    expect(wrapper?.querySelector("svg")).toBeInTheDocument();
  });

  it("defers favicon requests until a streaming message settles", () => {
    const markdown = "See https://docs.codemux.org for the rest.";
    const { container, rerender } = render(
      <ChatMarkdown streaming>{markdown}</ChatMarkdown>,
    );

    const wrapper = container.querySelector(".chat-markdown-link-favicon");
    expect(wrapper).not.toBeNull();
    expect(container.querySelector(".chat-markdown-link-favicon img")).toBeNull();
    expect(wrapper?.querySelector("svg")).toBeInTheDocument();

    rerender(<ChatMarkdown streaming={false}>{markdown}</ChatMarkdown>);

    expect(container.querySelector(".chat-markdown-link-favicon img")).toHaveAttribute(
      "src",
      "https://www.google.com/s2/favicons?domain=docs.codemux.org&sz=32",
    );
  });
});

describe("ChatMarkdown non-web link destinations", () => {
  // `ChatMarkdown` supplies its own rehype plugins, which replaces Streamdown's
  // sanitize/harden chain, so an agent-authored href reaches the anchor exactly
  // as written. Nothing but a confirmed http(s) URL may ever be navigable.
  it("renders a script-scheme destination inert", () => {
    const { container } = render(
      <ChatMarkdown>{"[click me](javascript:alert(1))"}</ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    expect(link).toHaveTextContent("click me");
    expect(link).not.toHaveAttribute("href");
    expect(link).toHaveAttribute("data-inert-link");
    // No href to follow, and the click is swallowed rather than defaulted.
    expect(fireEvent.click(link as HTMLAnchorElement)).toBe(false);

    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(mocks.openUrl).not.toHaveBeenCalled();
  });

  it("does not navigate the webview for a local path link", () => {
    const { container } = render(
      <ChatMarkdown>{"[log](/tmp/build.log)"}</ChatMarkdown>,
    );

    const link = container.querySelector('[data-streamdown="link"]');
    expect(link).toHaveTextContent("log");
    // A live href here would unload the single-page app onto app-origin/tmp/…
    expect(link).not.toHaveAttribute("href");
    expect(link).toHaveAttribute("title", "/tmp/build.log");
    expect(fireEvent.click(link as HTMLAnchorElement)).toBe(false);
    expect(middleClick(link as HTMLAnchorElement)).toBe(false);

    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(mocks.openUrl).not.toHaveBeenCalled();
  });

  it("leaves mailto, file, and relative destinations without a live href", () => {
    const { container } = render(
      <ChatMarkdown>
        {"[mail](mailto:hello@codemux.org) [file](file:///etc/hosts) [rel](./notes.md) [anchor](#code-blocks)"}
      </ChatMarkdown>,
    );

    const links = container.querySelectorAll('[data-streamdown="link"]');
    expect(links).toHaveLength(4);
    for (const link of links) {
      expect(link).not.toHaveAttribute("href");
      expect(link).toHaveAttribute("data-inert-link");
    }
  });
});

describe("ChatMarkdown local image links", () => {
  it("upgrades a labelled absolute PNG link to a clickable preview", () => {
    const { container, getByRole } = render(
      <ChatMarkdown>
        {"[Terminal screenshot](/home/me/screenshots/terminal.png)"}
      </ChatMarkdown>,
    );

    expect(container.querySelector("[data-chat-local-image]")).toBeInTheDocument();
    expect(container.querySelector("a")).toBeNull();
    expect(
      getByRole("button", { name: "Open Terminal screenshot" }),
    ).toBeInTheDocument();
  });

  it("upgrades absolute Markdown image syntax to the same preview", () => {
    const { container, getByRole } = render(
      <ChatMarkdown>{"![Agent Chat unchanged](/tmp/chat.webp)"}</ChatMarkdown>,
    );

    expect(container.querySelector("[data-chat-local-image]")).toBeInTheDocument();
    expect(
      getByRole("button", { name: "Open Agent Chat unchanged" }),
    ).toBeInTheDocument();
  });

  it("keeps a local file reference inert when no workspace root is available", () => {
    const { container } = render(
      <ChatMarkdown>{"[implementation](/home/me/src/app.tsx)"}</ChatMarkdown>,
    );

    expect(container.querySelector("[data-chat-local-image]")).toBeNull();
    expect(container.querySelector("button")).toBeNull();
    // Falls back to the inert-link treatment every non-web destination gets:
    // nothing to click, but the path still shows on hover.
    const link = container.querySelector('[data-streamdown="link"]');
    expect(link).toHaveTextContent("implementation");
    expect(link).not.toHaveAttribute("href");
    expect(link).toHaveAttribute("title", "/home/me/src/app.tsx");
  });
});

describe("ChatMarkdown source references", () => {
  const workspaceId = "ws-source-link";
  const cwd = "/work/codemux";

  it("opens an explicit Markdown file reference in the right panel at its line", () => {
    const { getByRole } = render(
      <ChatMarkdown workspaceId={workspaceId} cwd={cwd}>
        {"See [types.ts:42](src/lib/types.ts:42) for the contract."}
      </ChatMarkdown>,
    );

    const sourceLink = getByRole("button", { name: "types.ts:42" });
    fireEvent.click(sourceLink);

    const filePath = "/work/codemux/src/lib/types.ts";
    expect(useUIStore.getState().getRightPanelTab(workspaceId)).toBe(
      docPaneId(filePath),
    );
    expect(
      useEditorStore.getState().getTab(docEditorTabId(workspaceId, filePath)),
    ).toMatchObject({
      filePath,
      revealRequest: { line: 42, nonce: 1 },
    });

    fireEvent.click(sourceLink);
    expect(
      useEditorStore.getState().getTab(docEditorTabId(workspaceId, filePath)),
    ).toMatchObject({ revealRequest: { line: 42, nonce: 2 } });
  });

  it("keeps an inline-code file label as one accessible source button", () => {
    const { getAllByRole } = render(
      <ChatMarkdown workspaceId={workspaceId} cwd={cwd}>
        {"See [`types.ts:42`](src/lib/types.ts:42) for the contract."}
      </ChatMarkdown>,
    );

    const sourceLinks = getAllByRole("button", { name: "types.ts:42" });
    expect(sourceLinks).toHaveLength(1);
    expect(sourceLinks[0]?.querySelector("button")).toBeNull();
    expect(sourceLinks[0]).not.toHaveClass("border");
    expect(sourceLinks[0]?.className).not.toContain("bg-muted");
  });

  it("upgrades inline-code file references but leaves version-like code plain", () => {
    const { container, getByRole } = render(
      <ChatMarkdown workspaceId={workspaceId} cwd={cwd}>
        {"Changed `src/main.rs:18` while retaining `v1.2.3`."}
      </ChatMarkdown>,
    );

    expect(getByRole("button", { name: "src/main.rs:18" })).toBeInTheDocument();
    expect(container.querySelectorAll('code[data-streamdown="inline-code"]')).toHaveLength(1);
    expect(container.querySelector('code[data-streamdown="inline-code"]')).toHaveTextContent(
      "v1.2.3",
    );
  });

  it.each([
    "cargo check --manifest-path src-tauri/Cargo.toml",
    "npx vitest run src/components/chat/ChatMarkdown.links.test.tsx",
    "cat src/foo.ts",
  ])("leaves the command span `%s` as plain inline code", (command) => {
    const { container, queryByRole } = render(
      <ChatMarkdown workspaceId={workspaceId} cwd={cwd}>
        {`Run \`${command}\` first.`}
      </ChatMarkdown>,
    );

    expect(queryByRole("button")).toBeNull();
    expect(container.querySelector('code[data-streamdown="inline-code"]')).toHaveTextContent(
      command,
    );
  });

  it("makes a resolvable fenced-code title open the same source pane", () => {
    const { getByRole } = render(
      <TooltipProvider>
        <ChatMarkdown workspaceId={workspaceId} cwd={cwd}>
          {"```ts title=src/lib/types.ts:7\nexport type Item = string;\n```"}
        </ChatMarkdown>
      </TooltipProvider>,
    );

    fireEvent.click(getByRole("button", { name: "src/lib/types.ts:7" }));

    const filePath = "/work/codemux/src/lib/types.ts";
    expect(useUIStore.getState().getRightPanelTab(workspaceId)).toBe(
      docPaneId(filePath),
    );
    expect(
      useEditorStore.getState().getTab(docEditorTabId(workspaceId, filePath))
        ?.revealRequest?.line,
    ).toBe(7);
  });
});
