/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "./ImageLightbox";

describe("ImageLightbox", () => {
  afterEach(cleanup);

  it("shrink-wraps the image and exposes clear dismissal controls", () => {
    const onOpenChange = vi.fn();
    render(
      <ImageLightbox
        open
        onOpenChange={onOpenChange}
        title="Screenshot"
      >
        <img
          src="data:image/png;base64,AQID"
          alt="Screenshot"
          className={IMAGE_LIGHTBOX_MEDIA_CLASS}
        />
      </ImageLightbox>,
    );

    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveClass("w-fit");
    expect(dialog).not.toHaveClass("w-full");
    expect(document.querySelector('[data-slot="dialog-overlay"]')).toHaveClass(
      "bg-black/75",
    );
    expect(
      screen.getByRole("button", { name: "Close image preview" }),
    ).toBeVisible();

    fireEvent.click(
      screen.getByRole("button", { name: "Close expanded image" }),
    );
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("returns focus to the element that opened it", async () => {
    function Harness({ open }: { open: boolean }) {
      return (
        <>
          <button type="button">Open image</button>
          <ImageLightbox open={open} onOpenChange={() => {}} title="Screenshot">
            <img src="data:image/png;base64,AQID" alt="Screenshot" />
          </ImageLightbox>
        </>
      );
    }
    const view = render(<Harness open={false} />);
    const opener = screen.getByRole("button", { name: "Open image" });
    opener.focus();
    view.rerender(<Harness open />);
    expect(document.activeElement).not.toBe(opener);
    view.rerender(<Harness open={false} />);
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });
});
