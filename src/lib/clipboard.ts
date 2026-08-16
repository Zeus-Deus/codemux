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
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  } finally {
    if (previous instanceof HTMLElement) previous.focus();
  }
}
