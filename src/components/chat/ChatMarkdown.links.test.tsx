/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

import { ChatMarkdown } from "./ChatMarkdown";

afterEach(() => cleanup());

describe("ChatMarkdown rich external links", () => {
  it("adds the destination favicon to any labelled http(s) link", () => {
    const { container } = render(
      <ChatMarkdown>
        {"Committed and shipped: [PR #235](https://github.com/pingdotgg/t3code/pull/235)."}
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
});
