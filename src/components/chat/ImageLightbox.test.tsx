/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "./ImageLightbox";

describe("ImageLightbox", () => {
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
});
