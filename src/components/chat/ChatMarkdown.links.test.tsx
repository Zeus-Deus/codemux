/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

import { ChatMarkdown } from "./ChatMarkdown";
import { resetFaviconFailureCache } from "./MarkdownLinkFavicon";

beforeEach(() => resetFaviconFailureCache());
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
