/// <reference types="@testing-library/jest-dom/vitest" />
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectScreen, ConnectingView, pairingNotice } from "./bootstrap";

afterEach(cleanup);

describe("ConnectingView", () => {
  it("lets a waiting browser give up and says when the request expires", () => {
    const onCancel = vi.fn();
    render(<ConnectingView host="192.168.1.42:4377" waiting onCancel={onCancel} />);

    expect(screen.getByText("Waiting for approval")).toBeInTheDocument();
    expect(screen.getByText(/expires after 5 minutes/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /use a different code/i }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});

describe("pairingNotice", () => {
  it("explains a decline, a timeout and a lost pairing, and stays quiet on cancel", () => {
    expect(pairingNotice("unauthorized", true)).toMatch(/declined/);
    expect(pairingNotice("unauthorized", false)).toMatch(/no longer paired/);
    expect(pairingNotice("timed-out", true)).toMatch(/in time/);
    expect(pairingNotice("cancelled", true)).toBeNull();
  });
});

describe("ConnectScreen", () => {
  it("shows why it is back on the code form", () => {
    render(
      <ConnectScreen
        baseUrl="http://192.168.1.42:4377"
        host="192.168.1.42:4377"
        methods={{ pairingCode: true, account: false }}
        notice="The desktop declined this browser."
        onPaired={() => {}}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "The desktop declined this browser.",
    );
  });
});
