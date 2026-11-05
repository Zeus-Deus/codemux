import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { SidebarStickyHeader } from "./sidebar-sticky-header";

afterEach(cleanup);

function renderInScroller() {
  return render(
    <div data-sidebar="content">
      <SidebarStickyHeader>
        <div>header</div>
      </SidebarStickyHeader>
      <div>list</div>
    </div>,
  );
}

describe("SidebarStickyHeader", () => {
  it("reserves its height as the scroller's scroll-padding, and releases it on unmount", () => {
    const { container, unmount } = renderInScroller();
    const scroller = container.querySelector<HTMLElement>('[data-sidebar="content"]')!;
    expect(scroller.style.getPropertyValue("--sidebar-sticky-h")).toBe("0px");
    expect(scroller.style.scrollPaddingTop).toBe("calc(var(--sidebar-sticky-h) + 8px)");
    expect(scroller.style.scrollPaddingBottom).toBe("8px");

    unmount();
    expect(scroller.style.getPropertyValue("--sidebar-sticky-h")).toBe("");
    expect(scroller.style.scrollPaddingTop).toBe("");
  });

  it("shows the edge fade only while the list is scrolled", () => {
    const { container } = renderInScroller();
    const scroller = container.querySelector<HTMLElement>('[data-sidebar="content"]')!;
    const fade = container.querySelector('[data-scroll-edge="top"]')!;
    expect(fade).toHaveAttribute("data-visible", "false");

    act(() => {
      scroller.scrollTop = 40;
      fireEvent.scroll(scroller);
    });
    expect(fade).toHaveAttribute("data-visible", "true");
  });
});
