import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { Sidebar, SidebarGroupLabel, SidebarProvider, SidebarRail, SidebarTrigger } from "./sidebar"

const mobile = vi.hoisted(() => ({ value: false }))
vi.mock("@/hooks/use-mobile", () => ({ useIsMobile: () => mobile.value }))

afterEach(() => {
  cleanup()
  mobile.value = false
})

describe("desktop sidebar toggle", () => {
  it("commits a resized width and preserves it across collapse and expand", () => {
    const { container } = render(
      <SidebarProvider>
        <SidebarTrigger />
        <Sidebar collapsible="icon"><SidebarRail /></Sidebar>
      </SidebarProvider>
    )
    const wrapper = container.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')!
    fireEvent.pointerDown(screen.getByRole("separator"))
    expect(wrapper).toHaveAttribute("data-resizing", "true")
    fireEvent.pointerMove(window, { clientX: 350 })
    fireEvent.pointerUp(window)
    expect(wrapper).not.toHaveAttribute("data-resizing")
    expect(wrapper.style.getPropertyValue("--sidebar-width")).toBe("350px")
    fireEvent.click(screen.getByRole("button", { name: "Toggle Sidebar" }))
    fireEvent.click(screen.getByRole("button", { name: "Toggle Sidebar" }))
    expect(wrapper.style.getPropertyValue("--sidebar-width")).toBe("350px")
  })

  it("keeps the mobile Sheet toggle and its own animation", () => {
    mobile.value = true
    const { container } = render(
      <SidebarProvider>
        <SidebarTrigger />
        <Sidebar>Workspace</Sidebar>
      </SidebarProvider>
    )
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Toggle Sidebar" }))
    const sheet = screen.getByRole("dialog", { name: "Sidebar" })
    expect(sheet).toHaveAttribute("data-mobile", "true")
    expect(sheet).toHaveClass("data-open:animate-in", "data-closed:animate-out")
    expect(sheet).not.toHaveClass("transition-none")
    expect(container.querySelector('[data-slot="sidebar-gap"]')).toBeNull()
    fireEvent.keyDown(sheet, { key: "Escape" })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("keeps the label fade short and honors reduced motion", () => {
    render(<SidebarProvider><SidebarGroupLabel>Projects</SidebarGroupLabel></SidebarProvider>)
    expect(screen.getByText("Projects")).toHaveClass("duration-150", "ease-out", "motion-reduce:transition-none")
  })

  it.each(["icon", "offcanvas"] as const)("snaps the %s layout without a geometry transition", (collapsible) => {
    const { container } = render(
      <SidebarProvider>
        <SidebarTrigger />
        <Sidebar collapsible={collapsible}>Workspace</Sidebar>
      </SidebarProvider>
    )
    const sidebar = container.querySelector('[data-slot="sidebar"]')!
    const gap = container.querySelector('[data-slot="sidebar-gap"]')!
    const panel = container.querySelector('[data-slot="sidebar-container"]')!

    expect(sidebar).toHaveAttribute("data-state", "expanded")
    for (const element of [gap, panel]) {
      expect(element).toHaveClass("transition-none")
      expect(element).toHaveClass("group-data-[resizing]/sidebar-wrapper:!transition-none")
    }

    fireEvent.click(screen.getByRole("button", { name: "Toggle Sidebar" }))
    expect(sidebar).toHaveAttribute("data-state", "collapsed")
    expect(sidebar).toHaveAttribute("data-collapsible", collapsible)
    fireEvent.click(screen.getByRole("button", { name: "Toggle Sidebar" }))
    expect(sidebar).toHaveAttribute("data-state", "expanded")
  })
})

// The rail used to be pointer-only: no focus, no value, no way back to the
// default after an over-drag short of dragging precisely.
describe("sidebar rail keyboard and reset", () => {
  function renderRail() {
    const { container } = render(
      <SidebarProvider>
        <Sidebar collapsible="icon"><SidebarRail /></Sidebar>
      </SidebarProvider>
    )
    const wrapper = container.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')!
    const rail = screen.getByRole("separator", { name: "Resize sidebar" })
    return { rail, width: () => wrapper.style.getPropertyValue("--sidebar-width") }
  }

  it("is focusable and reports its width", () => {
    const { rail } = renderRail()
    expect(rail).toHaveAttribute("tabindex", "0")
    expect(rail).toHaveAttribute("aria-valuenow", "288")
    expect(rail).toHaveAttribute("aria-valuemin", "180")
    expect(rail).toHaveAttribute("aria-valuemax", "400")
  })

  it("resizes with the arrow keys, further with Shift", () => {
    const { rail, width } = renderRail()
    fireEvent.keyDown(rail, { key: "ArrowRight" })
    expect(width()).toBe("304px")
    fireEvent.keyDown(rail, { key: "ArrowLeft", shiftKey: true })
    expect(width()).toBe("240px")
    expect(rail).toHaveAttribute("aria-valuenow", "240")
  })

  it("jumps to the bounds with Home and End", () => {
    const { rail, width } = renderRail()
    fireEvent.keyDown(rail, { key: "End" })
    expect(width()).toBe("400px")
    fireEvent.keyDown(rail, { key: "Home" })
    expect(width()).toBe("180px")
  })

  it("resets to the default width on double-click", () => {
    const { rail, width } = renderRail()
    fireEvent.keyDown(rail, { key: "End" })
    fireEvent.doubleClick(rail)
    expect(width()).toBe("288px")
  })
})
