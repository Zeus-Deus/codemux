/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import { Spinner } from "./spinner";

afterEach(() => cleanup());

describe("Spinner", () => {
  it("announces a busy state with a default label", () => {
    render(<Spinner />);
    const spinner = screen.getByRole("status", { name: "Loading" });
    expect(spinner).toHaveAttribute("data-slot", "spinner");
    expect(spinner).toHaveClass("size-4", "animate-spin");
  });

  it("takes a specific label and an icon-ladder size", () => {
    render(<Spinner label="Syncing" className="size-3" />);
    const spinner = screen.getByRole("status", { name: "Syncing" });
    expect(spinner).toHaveClass("size-3");
    expect(spinner).not.toHaveClass("size-4");
  });
});
