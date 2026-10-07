import { useState, useEffect, useRef, useCallback } from "react";
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, highlightSpecialChars } from "@codemirror/view";
import { EditorState, Compartment, Transaction } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { Check, FileCode } from "lucide-react";
import { toast } from "sonner";
import { useEditorStore } from "@/stores/editor-store";
import { fileSignature, readFile, writeFile } from "@/tauri/commands";
import { buildEditorTheme } from "@/lib/codemirror-theme";
import {
  loadLanguage,
  isBinaryExtension,
  isImageExtension,
  isVideoExtension,
} from "@/lib/editor-languages";
import { useSyntaxThemeColors } from "@/hooks/use-theme-colors";
import { MarkdownRendered } from "./MarkdownRendered";
import { ImageViewer } from "./ImageViewer";
import { VideoViewer } from "./VideoViewer";
import { markPaneReady } from "@/lib/perf/interaction-trace";
import { PanelHeader } from "@/components/ui/panel-header";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface Props {
  tabId: string;
  /** Owning workspace for switch-trace attribution. Omitted in the right-panel
   *  document deck, which is not itself the switched main pane. */
  workspaceId?: string;
  /** Hosted inside the right-panel deck: the pane's own toolbar is
   *  suppressed because the deck's shared pane bar carries those controls
   *  (source toggle, wrap, copy, file tree) for every pane. */
  embedded?: boolean;
  /** Controlled rendered/raw mode. Omitted ⇒ the pane keeps its own
   *  state (markdown opens rendered, everything else raw). */
  viewMode?: EditorViewMode;
  /** Soft-wrap long lines. Defaults to on, which is what the main-area
   *  editor tab has always done. */
  wrap?: boolean;
}

export type EditorViewMode = "raw" | "rendered";
type ViewMode = EditorViewMode;

export function isMarkdownFile(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase();
  return ext === "md" || ext === "mdx" || ext === "markdown";
}

/** How often an open file is re-stat'ed for changes made outside the editor
 *  (agents, terminals, other tools). A stat is cheap; content is re-read only
 *  when the size or mtime moves. */
export const DISK_POLL_MS = 2000;
const NOTICE_MS = 1500;
/** Matches the `duration-150` surface fade on the notice. */
const NOTICE_FADE_MS = 150;

/** Swap the document for `next` by replacing only the span that differs, so
 *  the cursor, selection and scroll position outside an agent's edit stay
 *  where the user left them. */
function replaceDocument(view: EditorView, next: string, addToHistory: boolean) {
  const prev = view.state.doc.toString();
  const max = Math.min(prev.length, next.length);
  let start = 0;
  while (start < max && prev.charCodeAt(start) === next.charCodeAt(start)) start++;
  let prevEnd = prev.length;
  let nextEnd = next.length;
  while (
    prevEnd > start &&
    nextEnd > start &&
    prev.charCodeAt(prevEnd - 1) === next.charCodeAt(nextEnd - 1)
  ) {
    prevEnd--;
    nextEnd--;
  }
  view.dispatch({
    changes: { from: start, to: prevEnd, insert: next.slice(start, nextEnd) },
    annotations: Transaction.addToHistory.of(addToHistory),
  });
}

export function EditorPane({
  tabId,
  workspaceId,
  embedded = false,
  viewMode: viewModeProp,
  wrap = true,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const themeCompartment = useRef(new Compartment());
  const languageCompartment = useRef(new Compartment());
  const wrapCompartment = useRef(new Compartment());
  const isLoadingRef = useRef(false);

  const theme = useSyntaxThemeColors();

  const tab = useEditorStore((s) => s.getTab(tabId));
  const initTab = useEditorStore((s) => s.initTab);
  const setBaselineContent = useEditorStore((s) => s.setBaselineContent);
  const setDirty = useEditorStore((s) => s.setDirty);
  const clearReveal = useEditorStore((s) => s.clearReveal);

  const filePath = tab?.filePath ?? null;
  const isDirty = tab?.isDirty ?? false;
  const baselineContent = tab?.baselineContent ?? "";
  const revealRequest = tab?.revealRequest;
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [loadedFilePath, setLoadedFilePath] = useState<string | null>(null);
  /** Disk content that changed underneath unsaved edits, awaiting the
   *  user's Reload / Keep mine choice. */
  const [diskConflict, setDiskConflictState] = useState<string | null>(null);
  /** Mirrors `diskConflict` synchronously so a save fired in the same tick
   *  as the conflict appearing still sees it. */
  const diskConflictRef = useRef<string | null>(null);
  const setDiskConflict = useCallback((disk: string | null) => {
    diskConflictRef.current = disk;
    setDiskConflictState(disk);
  }, []);
  const [notice, setNotice] = useState<"saved" | "reloaded" | null>(null);
  const [noticeFading, setNoticeFading] = useState(false);
  /** Signature of the disk version the buffer was last reconciled with.
   *  `null` means unknown, so the next check re-reads and compares content. */
  const diskSignatureRef = useRef<string | null>(null);
  const checkingDiskRef = useRef(false);
  /** Bumped by every load and save, so a disk check that started before one
   *  of them drops its now-stale read instead of reverting the buffer. */
  const diskEpochRef = useRef(0);
  const noticeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isMd = filePath != null && isMarkdownFile(filePath);
  const isImage = filePath != null && isImageExtension(filePath);
  const isVideo = filePath != null && isVideoExtension(filePath);
  const [localViewMode, setViewMode] = useState<ViewMode>("raw");
  const viewMode = viewModeProp ?? localViewMode;

  useEffect(() => {
    if (
      filePath == null ||
      isImage ||
      isVideo ||
      errorMsg != null ||
      loadedFilePath === filePath
    ) {
      markPaneReady("editor", { target: workspaceId });
    }
  }, [errorMsg, filePath, isImage, isVideo, loadedFilePath, workspaceId]);

  // Default to rendered for markdown files when filePath changes
  useEffect(() => {
    if (filePath && isMarkdownFile(filePath)) {
      setViewMode("rendered");
    } else {
      setViewMode("raw");
    }
  }, [filePath]);

  // Initialize tab
  useEffect(() => {
    initTab(tabId);
  }, [tabId, initTab]);

  const flashNotice = useCallback((next: "saved" | "reloaded") => {
    if (noticeTimerRef.current) clearTimeout(noticeTimerRef.current);
    setNotice(next);
    setNoticeFading(false);
    noticeTimerRef.current = setTimeout(() => {
      setNoticeFading(true);
      noticeTimerRef.current = setTimeout(() => setNotice(null), NOTICE_FADE_MS);
    }, NOTICE_MS);
  }, []);

  useEffect(
    () => () => {
      if (noticeTimerRef.current) clearTimeout(noticeTimerRef.current);
    },
    [],
  );

  /** Replace the buffer with `disk` and make it the new clean baseline.
   *  `undoable` keeps the replaced text one Ctrl+Z away, so Reload can be
   *  taken back; a silent reload stays out of the history so Ctrl+Z cannot
   *  quietly revert the outside edit. */
  const adoptDiskContent = useCallback(
    (disk: string, undoable: boolean) => {
      const view = viewRef.current;
      if (view && view.state.doc.toString() !== disk) {
        isLoadingRef.current = true;
        try {
          replaceDocument(view, disk, undoable);
        } finally {
          isLoadingRef.current = false;
        }
      }
      setBaselineContent(tabId, disk);
      setContent(disk);
      setDiskConflict(null);
    },
    [tabId, setBaselineContent, setDiskConflict],
  );

  /** Reconcile the buffer with a disk version that differs from what it was
   *  loaded from: reload a clean buffer in place, but never touch unsaved
   *  edits without asking. */
  const reconcileDiskContent = useCallback(
    (disk: string) => {
      const tab = useEditorStore.getState().getTab(tabId);
      const view = viewRef.current;
      if (!tab || !view) return;
      if (disk === tab.baselineContent) {
        setDiskConflict(null);
        return;
      }
      if (view.state.doc.toString() === disk) {
        // The buffer already matches the new disk content: nothing to ask.
        setBaselineContent(tabId, disk);
        setContent(disk);
        setDiskConflict(null);
        return;
      }
      if (tab.isDirty) {
        setDiskConflict(disk);
        return;
      }
      adoptDiskContent(disk, false);
      flashNotice("reloaded");
    },
    [tabId, setBaselineContent, setDiskConflict, adoptDiskContent, flashNotice],
  );

  const checkDisk = useCallback(async () => {
    const path = useEditorStore.getState().getTab(tabId)?.filePath;
    if (!path || checkingDiskRef.current) return;
    checkingDiskRef.current = true;
    const epoch = diskEpochRef.current;
    try {
      // Stat before reading: a write that lands between the two moves the
      // signature again, so the next check still catches it.
      const signature = await fileSignature(path);
      if (signature === diskSignatureRef.current) return;
      const disk = await readFile(path);
      if (epoch !== diskEpochRef.current) return;
      if (useEditorStore.getState().getTab(tabId)?.filePath !== path) return;
      diskSignatureRef.current = signature;
      reconcileDiskContent(disk);
    } catch {
      // Deleted, renamed or briefly unreadable mid-write: keep the buffer
      // as it is and look again on the next tick.
    } finally {
      checkingDiskRef.current = false;
    }
  }, [tabId, reconcileDiskContent]);

  // Save handler
  const handleSave = useCallback(async () => {
    const view = viewRef.current;
    const path = useEditorStore.getState().getTab(tabId)?.filePath;
    if (!view || !path) return;

    // A pending conflict means disk holds a version the user has not
    // chosen to replace yet; Reload or Keep mine decides, not Ctrl+S.
    if (diskConflictRef.current != null) return;

    const c = view.state.doc.toString();
    try {
      // Refuse to silently overwrite a version written since this buffer
      // last synced with disk (an agent edit, typically).
      const signature = await fileSignature(path).catch(() => null);
      if (signature !== null && signature !== diskSignatureRef.current) {
        const disk = await readFile(path);
        if (useEditorStore.getState().getTab(tabId)?.filePath !== path) return;
        diskSignatureRef.current = signature;
        const baseline = useEditorStore.getState().getTab(tabId)?.baselineContent;
        if (disk !== baseline && disk !== c) {
          setDiskConflict(disk);
          return;
        }
      }
      diskEpochRef.current++;
      await writeFile(path, c);
      // Again after the write: a check that started while it was in flight
      // may have read the old (or half-written) file.
      diskEpochRef.current++;
      // The tab moved to another file while the write was in flight; that
      // file's load owns the baseline now.
      if (useEditorStore.getState().getTab(tabId)?.filePath !== path) return;
      // Unknown until the next check re-reads it, which also confirms the
      // write landed as this buffer.
      diskSignatureRef.current = null;
      setBaselineContent(tabId, c);
      setContent(c);
      setDiskConflict(null);
      flashNotice("saved");
    } catch (err) {
      toast.error(`Couldn't save ${path.split("/").pop() ?? path}`, {
        description: String(err),
      });
    }
  }, [tabId, setBaselineContent, setDiskConflict, flashNotice]);

  const reloadFromDisk = useCallback(() => {
    if (diskConflict == null) return;
    adoptDiskContent(diskConflict, true);
    flashNotice("reloaded");
  }, [diskConflict, adoptDiskContent, flashNotice]);

  /** Keep the unsaved edits and treat the newer disk version as the base,
   *  so the next save deliberately replaces it. */
  const keepMine = useCallback(() => {
    if (diskConflict == null) return;
    const view = viewRef.current;
    setBaselineContent(tabId, diskConflict);
    setDirty(tabId, view != null && view.state.doc.toString() !== diskConflict);
    setDiskConflict(null);
  }, [diskConflict, tabId, setBaselineContent, setDirty, setDiskConflict]);

  // Stable ref for save so keymap always calls the latest version
  const handleSaveRef = useRef(handleSave);
  handleSaveRef.current = handleSave;

  // Create CodeMirror instance
  useEffect(() => {
    if (!containerRef.current) return;

    const themeExt = themeCompartment.current.of(buildEditorTheme(theme));
    const langExt = languageCompartment.current.of([]);

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged && !isLoadingRef.current) {
        const c = update.state.doc.toString();
        const baseline = useEditorStore.getState().getTab(tabId)?.baselineContent ?? "";
        setDirty(tabId, c !== baseline);
        setContent(c);
      }
    });

    const saveBinding = keymap.of([
      {
        key: "Mod-s",
        run: () => {
          void handleSaveRef.current();
          return true;
        },
      },
    ]);

    const state = EditorState.create({
      doc: "",
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        drawSelection(),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        bracketMatching(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
        saveBinding,
        themeExt,
        langExt,
        updateListener,
        wrapCompartment.current.of(wrap ? EditorView.lineWrapping : []),
      ],
    });

    const view = new EditorView({ state, parent: containerRef.current });
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId]);

  // Wrap is a compartment so the deck's pane-bar toggle can flip it
  // without tearing the editor down and reloading the file.
  useEffect(() => {
    if (!viewRef.current) return;
    viewRef.current.dispatch({
      effects: wrapCompartment.current.reconfigure(
        wrap ? EditorView.lineWrapping : [],
      ),
    });
  }, [wrap]);

  // Update theme when it changes
  useEffect(() => {
    if (!viewRef.current) return;
    viewRef.current.dispatch({
      effects: themeCompartment.current.reconfigure(buildEditorTheme(theme)),
    });
  }, [theme]);

  // Load file content when filePath changes
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !filePath) return;

    // Previewable media renders in a dedicated viewer below \u2014 no need to read
    // bytes into the text editor or show a "binary" error.
    if (isImageExtension(filePath) || isVideoExtension(filePath)) {
      setErrorMsg(null);
      return;
    }

    if (isBinaryExtension(filePath)) {
      setErrorMsg("Binary file \u2014 cannot edit");
      return;
    }

    isLoadingRef.current = true;
    setErrorMsg(null);
    setLoadedFilePath(null);
    setDiskConflict(null);
    diskSignatureRef.current = null;
    diskEpochRef.current++;

    // Fire the file read IPC and the language-module dynamic import
    // concurrently. They are independent \u2014 `readFile` does Tauri IPC
    // for the file bytes, `loadLanguage` does an ESM dynamic import of
    // the appropriate `@codemirror/lang-*` package by extension.
    // Previously the language load was sequenced AFTER the read
    // resolved, which on a workspace switch with an editor tab open
    // doubled the wall clock (IPC round-trip + module load + parse)
    // for no reason \u2014 neither call needs the other's result.
    const readPromise = readFile(filePath);
    const langPromise = loadLanguage(filePath);

    readPromise
      .then((c) => {
        // Out of the undo history: Ctrl+Z must not empty a freshly opened file.
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: c },
          annotations: Transaction.addToHistory.of(false),
        });
        setBaselineContent(tabId, c);
        setContent(c);
        setLoadedFilePath(filePath);
      })
      .catch((err) => {
        setErrorMsg(String(err));
      })
      .finally(() => {
        isLoadingRef.current = false;
      });

    // Apply the language as soon as it's ready, independently of the
    // text content being in the document. CodeMirror's language
    // compartment can be reconfigured at any time; until it lands the
    // editor renders the file as plain text, which is fine because
    // the file is generally not yet visible (the rendered-markdown
    // path is the dominant view for .md files anyway).
    langPromise.then((lang) => {
      if (lang && viewRef.current) {
        viewRef.current.dispatch({
          effects: languageCompartment.current.reconfigure(lang),
        });
      }
    });
  }, [filePath, tabId, setBaselineContent, setDiskConflict]);

  // Watch the open text file for edits made outside the editor. Polling a
  // stat matches how the diff and changes panels refresh; checks also run
  // the moment the window regains focus or becomes visible.
  const watchDisk =
    filePath != null && loadedFilePath === filePath && errorMsg == null;
  useEffect(() => {
    if (!watchDisk) return;
    const check = () => {
      if (document.visibilityState === "visible") void checkDisk();
    };
    const timer = setInterval(check, DISK_POLL_MS);
    window.addEventListener("focus", check);
    document.addEventListener("visibilitychange", check);
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", check);
      document.removeEventListener("visibilitychange", check);
    };
  }, [watchDisk, checkDisk]);

  // Markdown opens rendered, which hides the CodeMirror container. When this
  // pane owns its mode (no controlled prop) it flips itself to source for a
  // pending line reveal; the right-panel deck drives its own flag instead, so
  // its rendered/source toggle keeps matching the pane.
  useEffect(() => {
    if (viewModeProp == null && isMd && revealRequest) setViewMode("raw");
  }, [isMd, revealRequest, viewModeProp]);

  // A source-reference click can target an already-open doc pane. Requests
  // carry a nonce so clicking the same citation twice still re-centres it,
  // and each is consumed once applied: `loadedFilePath` flips to the current
  // file on every mount, and the right panel mounts only the active pane, so
  // an unconsumed request would replay its cursor/scroll/focus reset on every
  // tab switch back.
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !revealRequest || loadedFilePath !== filePath) return;
    // A rendered-markdown pane keeps the CodeMirror container hidden, so a
    // scroll dispatched now would land nowhere. Hold the request instead —
    // the doc pane flips to raw whenever a line is requested.
    if (isMd && viewMode !== "raw") return;
    const lineNumber = Math.min(
      Math.max(1, revealRequest.line),
      view.state.doc.lines,
    );
    const line = view.state.doc.line(lineNumber);
    const columnOffset = Math.max(0, (revealRequest.column ?? 1) - 1);
    const position = Math.min(line.to, line.from + columnOffset);
    view.dispatch({
      selection: { anchor: position },
      effects: EditorView.scrollIntoView(position, { y: "center" }),
    });
    view.focus();
    clearReveal(tabId, revealRequest.nonce);
  }, [
    clearReveal,
    filePath,
    isMd,
    loadedFilePath,
    revealRequest,
    tabId,
    viewMode,
  ]);

  // When switching back to raw, sync content from store in case it changed
  useEffect(() => {
    if (viewMode === "raw" && viewRef.current && isMd) {
      const view = viewRef.current;
      const current = view.state.doc.toString();
      if (current !== content && content) {
        isLoadingRef.current = true;
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: content },
        });
        isLoadingRef.current = false;
      }
      // Focus the editor when switching to raw
      view.focus();
    }
  }, [viewMode, isMd, content]);

  if (!filePath) {
    return (
      <div className="flex h-full w-full items-center justify-center text-muted-foreground">
        <div className="flex flex-col items-center gap-2">
          <FileCode className="size-8 opacity-40" />
          <span className="text-label">Open a file from the file tree</span>
        </div>
      </div>
    );
  }

  if (errorMsg) {
    return (
      <div className="flex h-full w-full flex-col">
        <PanelHeader className={cn(embedded && "hidden", "gap-1 bg-card px-2")}>
          <span className="text-label font-mono text-muted-foreground truncate">
            {filePath}
          </span>
        </PanelHeader>
        <div className="flex flex-1 items-center justify-center text-muted-foreground">
          <span className="text-label">{errorMsg}</span>
        </div>
      </div>
    );
  }

  // Content to render: if dirty use live editor content, otherwise baseline
  const renderedContent = isDirty ? content : baselineContent;

  // Sits in the header; an embedded pane hides that header, so there the
  // chip floats over the top-right corner of the content instead.
  const noticeChip = notice != null && !isDirty && (
    <span
      role="status"
      className={cn(
        "flex shrink-0 items-center gap-1 text-caption text-muted-foreground",
        "transition-opacity duration-150 motion-reduce:transition-none",
        embedded
          ? "pointer-events-none absolute right-3 top-1.5 z-10 rounded-sm border border-hairline bg-popover px-1.5 py-0.5 shadow-sm"
          : "ml-1",
        noticeFading && "opacity-0",
      )}
    >
      {notice === "saved" && <Check className="size-3" />}
      {notice === "saved" ? "Saved" : "Reloaded from disk"}
    </span>
  );

  return (
    <div className="relative flex h-full w-full flex-col">
      {embedded && noticeChip}
      {/* Toolbar — suppressed in the deck, whose shared pane bar owns
          the path crumb and the source/wrap/copy controls. */}
      <PanelHeader className={cn(embedded && "hidden", "gap-1 bg-card px-2")}>
        <span className="text-label font-mono text-muted-foreground truncate min-w-0">
          {filePath}
        </span>
        {isDirty && viewMode === "raw" && (
          <span className="size-1.5 rounded-full bg-foreground/50 shrink-0 ml-1" title="Unsaved changes" />
        )}
        {!embedded && noticeChip}
        <div className="flex-1" />

        {/* View mode toggle — markdown files only */}
        {isMd && (
          <div className="flex items-center rounded-sm border border-border/50 overflow-hidden mr-1">
            <button
              className={`px-1.5 py-0.5 text-caption transition-colors ${viewMode === "rendered" ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground"}`}
              onClick={() => setViewMode("rendered")}
            >
              Rendered
            </button>
            <button
              className={`px-1.5 py-0.5 text-caption transition-colors ${viewMode === "raw" ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground"}`}
              onClick={() => setViewMode("raw")}
            >
              Raw
            </button>
          </div>
        )}

        {isDirty && viewMode === "raw" && (
          <span className="text-caption text-muted-foreground">
            Ctrl+S to save
          </span>
        )}
      </PanelHeader>

      {diskConflict != null && (
        <div
          role="alert"
          className="flex shrink-0 items-center gap-2 border-b border-hairline bg-surface-1 px-2 py-1"
        >
          {/* Wraps rather than truncates: a narrow deck must still say why
              the buttons are there. */}
          <span className="min-w-0 flex-1 text-label text-foreground">
            Changed on disk. Your edits are unsaved.
          </span>
          <Button size="xs" variant="outline" className="shrink-0" onClick={reloadFromDisk}>
            Reload
          </Button>
          <Button
            size="xs"
            variant="ghost"
            className="shrink-0"
            onClick={keepMine}
            title="Keep your edits. Saving will replace the version on disk."
          >
            Keep mine
          </Button>
        </div>
      )}

      {/* Image viewer — shown instead of the text editor for image files */}
      {isImage && <ImageViewer key={filePath} filePath={filePath} />}

      {/* Video viewer — stream local recordings through Tauri's asset URL. */}
      {isVideo && <VideoViewer key={filePath} filePath={filePath} />}

      {/* Rendered markdown view */}
      {isMd && viewMode === "rendered" && (
        <MarkdownRendered content={renderedContent} filePath={filePath} />
      )}

      {/* CodeMirror container — hidden for rendered or media previews */}
      <div
        ref={containerRef}
        className="flex-1 min-h-0 overflow-hidden [&_.cm-editor]:h-full [&_.cm-scroller]:overflow-auto"
        style={{
          display:
            isImage || isVideo || (isMd && viewMode === "rendered")
              ? "none"
              : undefined,
        }}
      />
    </div>
  );
}
