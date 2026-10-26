// Frozen external v1 artifact: change the host, not this fixture.
export default {
  id: "frozen-v1",
  name: "Frozen renderer API v1",
  version: "1.0.0",
  register(ctx) {
    const previous = ctx.storage.get("activations", 0);
    ctx.storage.set("activations", previous + 1);
    ctx.register({ id: "hello", area: "palette.commands", data: {
      label: "Frozen v1: hello",
      run() { ctx.host.notify({ message: "Frozen v1 works" }); },
    } });
    ctx.onDispose(() => ctx.storage.set("lastDisposed", true));
  },
};
