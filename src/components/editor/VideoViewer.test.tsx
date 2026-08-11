/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `mock-asset:${path}`,
}));

import { VideoViewer } from "./VideoViewer";

afterEach(cleanup);

describe("VideoViewer", () => {
  it("streams the converted local URL through a native video element", () => {
    const { container } = render(
      <VideoViewer filePath="/abs/scroll-performance-after.mp4" />,
    );
    const video = container.querySelector("video");

    expect(video).not.toBeNull();
    expect(video).toHaveAttribute(
      "src",
      "mock-asset:/abs/scroll-performance-after.mp4",
    );
    expect(video).toHaveAttribute("controls");
    expect(video).toHaveAttribute("autoplay");
    expect(video).toHaveAttribute("playsinline");
    expect(video).toHaveAttribute("preload", "metadata");
  });

  it("reveals the player once the webview can play it", () => {
    const { container } = render(<VideoViewer filePath="/abs/demo.webm" />);
    const video = container.querySelector("video")!;
    expect(video).toHaveClass("opacity-0");

    act(() => video.dispatchEvent(new Event("canplay")));

    expect(video).toHaveClass("opacity-100");
    expect(screen.queryByTestId("video-loading")).toBeNull();
  });

  it("shows a useful codec fallback when playback fails", () => {
    const { container } = render(<VideoViewer filePath="/abs/demo.mp4" />);
    const video = container.querySelector("video")!;

    act(() => video.dispatchEvent(new Event("error")));

    expect(screen.getByText("Can’t play this video")).toBeInTheDocument();
    expect(screen.getByText(/codec.*system webview/i)).toBeInTheDocument();
    expect(container.querySelector("video")).toBeNull();
  });
});
