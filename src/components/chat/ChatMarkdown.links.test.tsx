/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

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
});
afterEach(() => cleanup());

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
    expect(container.querySelector('[data-streamdown="link"]')).toBeNull();
    expect(container.querySelector('code[data-streamdown="inline-code"]')).toHaveTextContent(
      "implementation",
    );
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
