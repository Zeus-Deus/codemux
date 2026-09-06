import { useState } from "react";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { ImageLightbox } from "./ImageLightbox";
import { TranscriptCacheMount, TranscriptCacheProvider } from "./transcript-cache";

afterEach(cleanup);
function ImageRow() {
  const [open, setOpen] = useState(false);
  return <><button onClick={() => setOpen(true)}>Preview</button>
    <ImageLightbox open={open} onOpenChange={setOpen} title="Test image"><span>Image</span></ImageLightbox>
  </>;
}
function tree(active: string) {
  return <><button data-testid="outside">Workspace</button>
    <TranscriptCacheProvider activeKey={active} validKeys={["a", "b"]}>
      <TranscriptCacheMount key={active} cacheKey={active}><ImageRow /></TranscriptCacheMount>
    </TranscriptCacheProvider>
  </>;
}
it("does not resurrect a portaled image dialog or steal focus after a keyboard workspace switch", async () => {
  const view = render(tree("a"));
  fireEvent.click(view.getByText("Preview"));
  expect(view.getByRole("dialog")).toBeTruthy();
  // A keyboard/control switch need not trigger Radix's pointer-outside dismissal.
  view.rerender(tree("b"));
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
  const outside = view.getByTestId("outside");
  outside.focus();
  view.rerender(tree("a"));
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
  expect(view.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(outside);
});
