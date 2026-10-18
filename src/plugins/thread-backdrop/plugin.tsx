/**
 * Thread backdrop: one of the sidebar's engravings, rebuilt from monospace
 * type behind the new-thread landing (both a fresh draft and an existing
 * thread with no messages). Official, on by default; turning it off removes
 * it completely and the landing is exactly the plain one.
 */
import { definePlugin, LANDING_BACKDROP_AREA, type LandingBackdropProps } from "@codemux/sdk";
import { Backdrop } from "./Backdrop";

export default definePlugin({
  id: "thread-backdrop",
  name: "Thread backdrop",
  register(ctx) {
    // Created once per incarnation so the render identity stays stable.
    const render = (props: LandingBackdropProps) => <Backdrop {...props} host={ctx.host} />;
    ctx.register({ id: "backdrop", area: LANDING_BACKDROP_AREA, render });
  },
});
