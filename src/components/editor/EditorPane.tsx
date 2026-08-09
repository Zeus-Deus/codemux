import { useState, useEffect, useRef, useCallback } from "react";
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, highlightSpecialChars } from "@codemirror/view";
import { EditorState, Compartment } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput } from "@codemirror/language";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { FileCode } from "lucide-react";
import { useEditorStore } from "@/stores/editor-store";
import { readFile, writeFile } from "@/tauri/commands";
import { buildEditorTheme } from "@/lib/codemirror-theme";
import { loadLanguage, isBinaryExtension, isImageExtension } from "@/lib/editor-languages";
import { useSyntaxThemeColors } from "@/hooks/use-theme-colors";
import { MarkdownRendered } from "./MarkdownRendered";
import { ImageViewer } from "./ImageViewer";

interface Props {
  tabId: string;
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

export function EditorPane({ tabId, embedded = false, viewMode: viewModeProp, wrap = true }: Props) {
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

  const isMd = filePath != null && isMarkdownFile(filePath);
  const isImage = filePath != null && isImageExtension(filePath);
  const [localViewMode, setViewMode] = useState<ViewMode>("raw");
  const viewMode = viewModeProp ?? localViewMode;

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

  // Save handler
  const handleSave = useCallback(() => {
    const view = viewRef.current;
    const path = useEditorStore.getState().getTab(tabId)?.filePath;
    if (!view || !path) return;

    const c = view.state.doc.toString();
    writeFile(path, c)
      .then(() => {
        setBaselineContent(tabId, c);
        setContent(c);
      })
      .catch((err) => {
        console.error("Failed to save:", err);
      });
  }, [tabId, setBaselineContent]);

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
          handleSaveRef.current();
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

    // Images render in a dedicated viewer below \u2014 no need to read
    // bytes into the text editor or show a "binary" error.
    if (isImageExtension(filePath)) {
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
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: c },
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
  }, [filePath, tabId, setBaselineContent]);

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
          <FileCode className="h-8 w-8 opacity-40" />
          <span className="text-xs">Open a file from the file tree</span>
        </div>
      </div>
    );
  }

  if (errorMsg) {
    return (
      <div className="flex h-full w-full flex-col">
        <div className={`${embedded ? "hidden" : "flex"} h-7 shrink-0 items-center gap-1 border-b border-border/30 bg-card px-2`}>
          <span className="text-xs font-mono text-muted-foreground truncate">
            {filePath}
          </span>
        </div>
        <div className="flex flex-1 items-center justify-center text-muted-foreground">
          <span className="text-xs">{errorMsg}</span>
        </div>
      </div>
    );
  }

  // Content to render: if dirty use live editor content, otherwise baseline
  const renderedContent = isDirty ? content : baselineContent;

  return (
    <div className="flex h-full w-full flex-col">
      {/* Toolbar — suppressed in the deck, whose shared pane bar owns
          the path crumb and the source/wrap/copy controls. */}
      <div className={`${embedded ? "hidden" : "flex"} h-7 shrink-0 items-center gap-1 border-b border-border/30 bg-card px-2`}>
        <span className="text-xs font-mono text-muted-foreground truncate min-w-0">
          {filePath}
        </span>
        {isDirty && viewMode === "raw" && (
          <span className="w-1.5 h-1.5 rounded-full bg-foreground/50 shrink-0 ml-1" title="Unsaved changes" />
        )}
        <div className="flex-1" />

        {/* View mode toggle — markdown files only */}
        {isMd && (
          <div className="flex items-center rounded-sm border border-border/50 overflow-hidden mr-1">
            <button
              className={`px-1.5 py-0.5 text-[10px] transition-colors ${viewMode === "rendered" ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground"}`}
              onClick={() => setViewMode("rendered")}
            >
              Rendered
            </button>
            <button
              className={`px-1.5 py-0.5 text-[10px] transition-colors ${viewMode === "raw" ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground"}`}
              onClick={() => setViewMode("raw")}
            >
              Raw
            </button>
          </div>
        )}

        {isDirty && viewMode === "raw" && (
          <span className="text-[10px] text-muted-foreground">
            Ctrl+S to save
          </span>
        )}
      </div>

      {/* Image viewer — shown instead of the text editor for image files */}
      {isImage && <ImageViewer filePath={filePath} />}

      {/* Rendered markdown view */}
      {isMd && viewMode === "rendered" && (
        <MarkdownRendered content={renderedContent} filePath={filePath} />
      )}

      {/* CodeMirror container — hidden when showing rendered view or image */}
      <div
        ref={containerRef}
        className="flex-1 min-h-0 overflow-hidden [&_.cm-editor]:h-full [&_.cm-scroller]:overflow-auto"
        style={{
          display:
            isImage || (isMd && viewMode === "rendered") ? "none" : undefined,
        }}
      />
    </div>
  );
}
