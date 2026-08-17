/**
 * What to tell the user when `copyToClipboard` resolves false. Shared so every
 * copy affordance fails the same way instead of inventing its own wording.
 */
export const COPY_FAILED_MESSAGE = "Couldn't copy — select and copy manually.";

/**
 * Write `text` to the system clipboard, resolving to whether it landed.
 *
 * The async Clipboard API is the fast path, but Codemux also renders in the
 * remote web client, which can be served from a plain-HTTP origin where
 * `navigator.clipboard` is undefined. Fall back to a throwaway off-screen
 * textarea + `document.execCommand("copy")` there so copy still works.
 *
 * The fallback has to focus the textarea to select it, so the previously
 * focused element (usually the composer) is restored afterwards — losing the
 * caret mid-conversation just to copy a message would be worse than the
 * copy failing.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (window.isSecureContext && navigator?.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the execCommand path */
  }

  const previous = document.activeElement;
  const ta = document.createElement("textarea");
  try {
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    // Both of these have to run even when `execCommand` throws, or a failed
    // copy leaves a stray textarea in the DOM and the caret on it.
    ta.remove();
    if (previous instanceof HTMLElement) previous.focus();
  }
}
