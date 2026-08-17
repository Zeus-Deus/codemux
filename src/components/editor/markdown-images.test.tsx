/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockOpenExternalUrl = vi.fn().mockResolvedValue({ kind: "browser" });
vi.mock("@/lib/open-url", () => ({
  openExternalUrl: (...a: unknown[]) => mockOpenExternalUrl(...a),
}));

import { MarkdownRendered } from "./MarkdownRendered";

const SHOT = "https://example.com/before-after.png";
const DOC = `Here is the fix.\n\n![before and after](${SHOT})\n`;

afterEach(() => {
  cleanup();
  mockOpenExternalUrl.mockClear();
});

describe("embedded images in rendered markdown", () => {
  it("renders inline at a readable cap rather than at the file's own size", () => {
    render(<MarkdownRendered content={DOC} inline />);

    const img = screen.getByTestId("markdown-image");
    expect(img).toHaveAttribute("src", SHOT);
    expect(img).toHaveAttribute("alt", "before and after");
    expect(img.className).toContain("max-h-[360px]");
    expect(img.className).toContain("object-contain");
    expect(img.className).toContain("cursor-zoom-in");
  });

  it("opens a lightbox on click and closes it again on Escape", async () => {
    const user = userEvent.setup();
    render(<MarkdownRendered content={DOC} inline />);

    expect(screen.queryByTestId("image-lightbox")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("markdown-image"));

    const overlay = screen.getByTestId("image-lightbox");
    expect(overlay).toBeInTheDocument();
    expect(screen.getByTestId("image-lightbox-image")).toHaveAttribute("src", SHOT);

    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("image-lightbox")).not.toBeInTheDocument();
  });

  it("closes on a backdrop click but not on a click of the image itself", async () => {
    const user = userEvent.setup();
    render(<MarkdownRendered content={DOC} inline />);

    await user.click(screen.getByTestId("markdown-image"));
    // Clicking the picture is looking closer, not leaving.
    await user.click(screen.getByTestId("image-lightbox-image"));
    expect(screen.getByTestId("image-lightbox")).toBeInTheDocument();

    await user.click(screen.getByTestId("image-lightbox"));
    expect(screen.queryByTestId("image-lightbox")).not.toBeInTheDocument();
  });

  it("offers the original in the browser", async () => {
    const user = userEvent.setup();
    render(<MarkdownRendered content={DOC} inline />);

    await user.click(screen.getByTestId("markdown-image"));
    await user.click(screen.getByTestId("image-lightbox-open-external"));

    expect(mockOpenExternalUrl).toHaveBeenCalledWith(SHOT);
  });

  it("has nothing to open in a browser for a local asset", async () => {
    const user = userEvent.setup();
    render(<MarkdownRendered content={"![shot](./shot.png)"} filePath="/tmp/a/README.md" inline />);

    await user.click(screen.getByTestId("markdown-image"));
    expect(screen.getByTestId("image-lightbox")).toBeInTheDocument();
    expect(
      screen.queryByTestId("image-lightbox-open-external"),
    ).not.toBeInTheDocument();
  });
});
