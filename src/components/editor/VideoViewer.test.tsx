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

  // A webview honouring preload="metadata" can settle at HAVE_METADATA without
  // ever firing `canplay`, which used to strand the spinner over live controls.
  it.each(["loadedmetadata", "loadeddata"])(
    "reveals the player once %s arrives without canplay",
    (eventName) => {
      const { container } = render(<VideoViewer filePath="/abs/demo.webm" />);
      const video = container.querySelector("video")!;
      expect(video).toHaveClass("opacity-0");
      expect(screen.getByTestId("video-loading")).toBeInTheDocument();

      act(() => video.dispatchEvent(new Event(eventName)));

      expect(video).toHaveClass("opacity-100");
      expect(screen.queryByTestId("video-loading")).toBeNull();
    },
  );

  it("shows a useful codec fallback when playback fails", () => {
    const { container } = render(<VideoViewer filePath="/abs/demo.mp4" />);
    const video = container.querySelector("video")!;

    act(() => video.dispatchEvent(new Event("error")));

    expect(screen.getByText("Can’t play this video")).toBeInTheDocument();
    expect(screen.getByText(/codec.*system webview/i)).toBeInTheDocument();
    expect(container.querySelector("video")).toBeNull();
  });

  it("keeps the error state terminal when a late readiness event lands", () => {
    const { container } = render(<VideoViewer filePath="/abs/demo.mp4" />);
    const video = container.querySelector("video")!;

    act(() => video.dispatchEvent(new Event("error")));
    act(() => video.dispatchEvent(new Event("loadedmetadata")));

    expect(screen.getByText("Can’t play this video")).toBeInTheDocument();
    expect(container.querySelector("video")).toBeNull();
  });

  it("paints the error fallback on the theme background, not a hardcoded black", () => {
    const { container } = render(<VideoViewer filePath="/abs/demo.mp4" />);
    const video = container.querySelector("video")!;

    act(() => video.dispatchEvent(new Event("error")));

    const fallback = container.firstElementChild!;
    expect(fallback).toHaveClass("bg-[var(--background)]");
    expect(fallback.className).not.toContain("#050505");
  });

  it("keeps the cinema backdrop and light spinner on the video stage", () => {
    const { container } = render(<VideoViewer filePath="/abs/demo.webm" />);

    expect(screen.getByTestId("video-viewer")).toHaveClass("bg-[#050505]");
    // Theme-derived text can be dark; the spinner sits on the black stage.
    expect(container.querySelector(".animate-spin")).toHaveClass("text-white/60");
  });
});
