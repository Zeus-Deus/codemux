/**
 * Screen tests for the hosted bootstrap UI.
 *
 * The signed-out screen is the only Codemux surface reachable from the open web
 * without installing anything, so a visitor who lands on it with no account has
 * to be able to tell what the page is and where to go instead. These lock that
 * context in: without them the screen silently degrades back to a bare
 * credential prompt the next time the card is refactored.
 */
import { cleanup, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { HostedScreen } from "./hosted-bootstrap";
import { HostedFlow, type HostedState } from "./hosted";

function flow(): HostedFlow {
  return new HostedFlow({
    signIn: async () => [],
    connect: async () => {},
    beginGithubSignIn: () => {},
    completeGithubSignIn: async () => [],
  });
}

function state(over: Partial<HostedState> = {}): HostedState {
  return {
    phase: "signin",
    devices: [],
    selected: null,
    error: null,
    connectStatus: "connecting",
    busy: false,
    ...over,
  };
}

describe("HostedScreen", () => {
  // This suite has no globals-driven auto-cleanup; unmount between renders so
  // `screen` queries only ever see the screen under test.
  beforeEach(cleanup);

  it("names the product and says what the page is before asking for credentials", () => {
    render(
      <HostedScreen state={state()} flow={flow()} apiHost="api.codemux.org" />,
    );

    expect(screen.getByText("Codemux")).toBeInTheDocument();
    expect(
      screen.getByText(/web client for a desktop you.{1,3}ve connected/i),
    ).toBeInTheDocument();
  });

  it("offers a way out for a visitor with no account and no connected machine", () => {
    render(
      <HostedScreen state={state()} flow={flow()} apiHost="api.codemux.org" />,
    );

    const link = screen.getByRole("link", { name: /codemux\.org/ });
    expect(link).toHaveAttribute("href", "https://codemux.org");
    expect(screen.getByText("codemux connect")).toBeInTheDocument();
  });

  it("drops the signed-out context once the visitor is past sign-in", () => {
    render(
      <HostedScreen
        state={state({ phase: "connecting" })}
        flow={flow()}
        apiHost="api.codemux.org"
      />,
    );

    expect(screen.getByText("Codemux")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /codemux\.org/ })).toBeNull();
  });
});
