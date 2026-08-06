/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { MonitoringBar } from "./MonitoringBar";

afterEach(cleanup);

describe("MonitoringBar", () => {
  it("renders nothing unless the pane reports monitoring", () => {
    const { container } = render(
      <MonitoringBar monitoring={false} threadId="t1" onStop={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("shows the docked bar while monitoring", () => {
    render(<MonitoringBar monitoring threadId="t1" onStop={() => {}} />);
    expect(screen.getByTestId("monitoring-bar")).toBeInTheDocument();
    expect(
      screen.getByText("Monitoring in the background"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });

  // Calm background presence, not a call for attention.
  it("never animates its dot", () => {
    render(<MonitoringBar monitoring threadId="t1" onStop={() => {}} />);
    expect(screen.getByTestId("monitoring-bar-dot").className).not.toContain(
      "animate",
    );
  });

  it("surfaces the reason an agent supplied", () => {
    render(
      <MonitoringBar
        monitoring
        reason="CI checks on PR #482"
        threadId="t1"
        onStop={() => {}}
      />,
    );
    expect(screen.getByTestId("monitoring-bar-reason")).toHaveTextContent(
      "CI checks on PR #482",
    );
  });

  it("omits the reason slot entirely when none was given", () => {
    render(<MonitoringBar monitoring threadId="t1" onStop={() => {}} />);
    expect(screen.queryByTestId("monitoring-bar-reason")).toBeNull();
  });

  it("invokes the stop command and parks on Stopping…", async () => {
    let resolve: () => void = () => {};
    const onStop = vi.fn(
      () =>
        new Promise<void>((r) => {
          resolve = r;
        }),
    );
    render(<MonitoringBar monitoring threadId="t1" onStop={onStop} />);

    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledTimes(1);
    const button = screen.getByRole("button", { name: "Stopping…" });
    expect(button).toBeDisabled();

    // A second click while pending must not fire the command again.
    fireEvent.click(button);
    expect(onStop).toHaveBeenCalledTimes(1);
    resolve();
  });

  it("releases Stopping… when the status leaves monitoring, not when the promise settles", () => {
    const onStop = vi.fn(() => new Promise<void>(() => {}));
    const { rerender } = render(
      <MonitoringBar monitoring threadId="t1" onStop={onStop} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(screen.getByRole("button", { name: "Stopping…" })).toBeDisabled();

    // Backend published the recomputed status: the bar unmounts...
    rerender(
      <MonitoringBar monitoring={false} threadId="t1" onStop={onStop} />,
    );
    expect(screen.queryByTestId("monitoring-bar")).toBeNull();

    // ...and a later monitoring run starts from a clean Stop, not a stuck one.
    rerender(<MonitoringBar monitoring threadId="t1" onStop={onStop} />);
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
  });

  it("resets the pending state across a thread switch", () => {
    const onStop = vi.fn(() => new Promise<void>(() => {}));
    const { rerender } = render(
      <MonitoringBar monitoring threadId="t1" onStop={onStop} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(screen.getByRole("button", { name: "Stopping…" })).toBeDisabled();

    // A different thread that also happens to be monitoring must not inherit
    // the first thread's pending spinner.
    rerender(<MonitoringBar monitoring threadId="t2" onStop={onStop} />);
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
  });

  // A pane can read `monitoring` purely from a `codemux monitor start` flag,
  // with no chat thread bound to it at all. Stop has to work there — the bar
  // is the only UI that can clear that flag, and a handler that quietly
  // returns on a null thread leaves the button parked at "Stopping…" forever.
  it("still stops on a pane with no bound thread", async () => {
    const onStop = vi.fn(() => Promise.resolve());
    const { rerender } = render(
      <MonitoringBar
        monitoring
        reason="watching the deploy"
        threadId={null}
        onStop={onStop}
      />,
    );
    const button = screen.getByRole("button", { name: "Stop" });
    expect(button).toBeEnabled();
    fireEvent.click(button);
    expect(onStop).toHaveBeenCalledTimes(1);

    // And the pending state still releases on the status, so nothing wedges.
    rerender(
      <MonitoringBar monitoring={false} threadId={null} onStop={onStop} />,
    );
    expect(screen.queryByTestId("monitoring-bar")).toBeNull();
  });

  it("recovers the button when the stop command fails", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const onStop = vi.fn(() => Promise.reject(new Error("no session")));
    render(<MonitoringBar monitoring threadId="t1" onStop={onStop} />);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(await screen.findByRole("button", { name: "Stop" })).toBeEnabled();
    error.mockRestore();
  });
});
