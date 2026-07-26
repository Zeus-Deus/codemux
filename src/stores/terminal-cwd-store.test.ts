import { describe, it, expect, beforeEach } from "vitest";
import { useTerminalCwdStore, parseOsc7 } from "./terminal-cwd-store";

const store = () => useTerminalCwdStore.getState();

beforeEach(() => {
  useTerminalCwdStore.setState({ cwds: {} });
});

describe("terminal cwd store", () => {
  it("records a cwd from either source", () => {
    store().setCwd("s1", "/a/b", "osc7");
    store().setCwd("s2", "/c/d", "proc");
    expect(store().cwds.s1).toEqual({ cwd: "/a/b", source: "osc7" });
    expect(store().cwds.s2).toEqual({ cwd: "/c/d", source: "proc" });
  });

  describe("osc7-wins precedence", () => {
    it("rejects a proc write over an existing osc7 value", () => {
      store().setCwd("s1", "/from/osc7", "osc7");
      store().setCwd("s1", "/from/proc", "proc");
      expect(store().cwds.s1.cwd).toBe("/from/osc7");
    });

    it("rejects a polled batch over an existing osc7 value", () => {
      store().setCwd("s1", "/from/osc7", "osc7");
      store().setPolledCwds({ s1: "/from/proc" });
      expect(store().cwds.s1.cwd).toBe("/from/osc7");
    });

    it("lets osc7 upgrade an existing proc value", () => {
      store().setCwd("s1", "/from/proc", "proc");
      store().setCwd("s1", "/from/osc7", "osc7");
      expect(store().cwds.s1).toEqual({ cwd: "/from/osc7", source: "osc7" });
    });

    it("still lets proc update its own value", () => {
      store().setCwd("s1", "/one", "proc");
      store().setPolledCwds({ s1: "/two" });
      expect(store().cwds.s1.cwd).toBe("/two");
    });
  });

  describe("re-render avoidance", () => {
    it("keeps the same state object when a poll changes nothing", () => {
      store().setCwd("s1", "/a", "proc");
      const before = useTerminalCwdStore.getState().cwds;
      store().setPolledCwds({ s1: "/a" });
      expect(useTerminalCwdStore.getState().cwds).toBe(before);
    });

    it("keeps the same state object when setCwd is a no-op", () => {
      store().setCwd("s1", "/a", "proc");
      const before = useTerminalCwdStore.getState().cwds;
      store().setCwd("s1", "/a", "proc");
      expect(useTerminalCwdStore.getState().cwds).toBe(before);
    });

    it("produces a new object when a poll does change something", () => {
      store().setCwd("s1", "/a", "proc");
      const before = useTerminalCwdStore.getState().cwds;
      store().setPolledCwds({ s1: "/b" });
      expect(useTerminalCwdStore.getState().cwds).not.toBe(before);
    });
  });

  describe("osc7SessionIds", () => {
    it("lists only the osc7-backed sessions so the poller can skip them", () => {
      store().setCwd("s1", "/a", "osc7");
      store().setCwd("s2", "/b", "proc");
      store().setCwd("s3", "/c", "osc7");
      expect(store().osc7SessionIds()).toEqual(new Set(["s1", "s3"]));
    });
  });

  describe("clearCwd", () => {
    it("forgets a closed session", () => {
      store().setCwd("s1", "/a", "osc7");
      store().clearCwd("s1");
      expect(store().cwds.s1).toBeUndefined();
    });

    it("is a no-op for an unknown session", () => {
      const before = useTerminalCwdStore.getState().cwds;
      store().clearCwd("nope");
      expect(useTerminalCwdStore.getState().cwds).toBe(before);
    });
  });
});

describe("parseOsc7", () => {
  it("extracts the path from a file URI with a hostname", () => {
    expect(parseOsc7("file://myhost/home/zeus/proj")).toBe("/home/zeus/proj");
  });

  it("extracts the path from a file URI with an empty authority", () => {
    expect(parseOsc7("file:///home/zeus/proj")).toBe("/home/zeus/proj");
  });

  it("percent-decodes spaces and unicode", () => {
    expect(parseOsc7("file:///home/zeus/my%20docs")).toBe("/home/zeus/my docs");
    expect(parseOsc7("file:///tmp/caf%C3%A9")).toBe("/tmp/café");
  });

  it("accepts a bare absolute path (off-spec shells)", () => {
    expect(parseOsc7("/home/zeus")).toBe("/home/zeus");
  });

  it("returns null for a file URI with no path", () => {
    expect(parseOsc7("file://myhost")).toBeNull();
  });

  it("returns null for a non-file scheme or relative junk", () => {
    expect(parseOsc7("http://example.com/x")).toBeNull();
    expect(parseOsc7("not-a-path")).toBeNull();
    expect(parseOsc7("")).toBeNull();
  });

  it("returns null on malformed percent-encoding rather than mojibake", () => {
    expect(parseOsc7("file:///tmp/%E0%A4%A")).toBeNull();
  });
});
