/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { ContribBoundary, ContribRender } from "./boundary";

afterEach(cleanup);

const Broken = () => {
  throw new Error("boom");
};
const Healthy = () => <p>healthy pane</p>;

describe("ContribBoundary", () => {
  it("does not carry one contribution's crash over to the next", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const view = render(
      <ContribBoundary id="a:pane" variant="block">
        <ContribRender render={Broken} />
      </ContribBoundary>,
    );
    expect(screen.getByRole("alert")).toBeInTheDocument();
    view.rerender(
      <ContribBoundary id="b:pane" variant="block">
        <ContribRender render={Healthy} />
      </ContribBoundary>,
    );
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("healthy pane")).toBeInTheDocument();
  });
});
