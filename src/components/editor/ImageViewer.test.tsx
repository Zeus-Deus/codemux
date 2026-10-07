/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `mock-asset:${path}`,
}));

import { ImageViewer } from "./ImageViewer";

afterEach(() => cleanup());

describe("ImageViewer", () => {
  it("renders the image with a converted asset src", () => {
    const { container } = render(<ImageViewer filePath="/abs/photo.png" />);
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img!.getAttribute("src")).toBe("mock-asset:/abs/photo.png");
  });

  it("uses the file path as the alt text fallback", () => {
    const { container } = render(<ImageViewer filePath="/abs/photo.png" />);
    const img = container.querySelector("img");
    expect(img!.getAttribute("alt")).toBe("/abs/photo.png");
  });

  it("fades the image in once it has loaded", () => {
    const { container } = render(<ImageViewer filePath="/abs/photo.png" />);
    const img = container.querySelector("img")!;
    expect(img).toHaveClass("opacity-0", "transition-opacity", "duration-150");

    act(() => {
      img.dispatchEvent(new Event("load"));
    });

    expect(img).toHaveClass("opacity-100");
  });

  it("shows a fallback message if the image fails to load", () => {
    const { container, queryByText } = render(
      <ImageViewer filePath="/abs/photo.png" />,
    );
    const img = container.querySelector("img");
    expect(img).not.toBeNull();

    act(() => {
      img!.dispatchEvent(new Event("error"));
    });

    expect(queryByText(/Failed to load image/i)).toBeInTheDocument();
    // Image is removed after the error transitions to the fallback.
    expect(container.querySelector("img")).toBeNull();
  });
});
