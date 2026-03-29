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
import { loadLanguage, isBinaryExtension } from "@/lib/editor-languages";
import { useThemeColors } from "@/hooks/use-theme-colors";
import { MarkdownRendered } from "./MarkdownRendered";

interface Props {
  tabId: string;
}

type ViewMode = "raw" | "rendered";

function isMarkdownFile(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase();
  return ext === "md" || ext === "mdx" || ext === "markdown";
}

export function EditorPane({ tabId }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const themeCompartment = useRef(new Compartment());
  const languageCompartment = useRef(new Compartment());
  const isLoadingRef = useRef(false);

  const { theme } = useThemeColors();

  const tab = useEditorStore((s) => s.getTab(tabId));
  const initTab = useEditorStore((s) => s.initTab);
  const setBaselineContent = useEditorStore((s) => s.setBaselineContent);
  const setDirty = useEditorStore((s) => s.setDirty);

  const filePath = tab?.filePath ?? null;
  const isDirty = tab?.isDirty ?? false;
  const baselineContent = tab?.baselineContent ?? "";
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [content, setContent] = useState("");

  const isMd = filePath != null && isMarkdownFile(filePath);
  const [viewMode, setViewMode] = useState<ViewMode>("raw");

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
        EditorView.lineWrapping,
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

    if (isBinaryExtension(filePath)) {
      setErrorMsg("Binary file \u2014 cannot edit");
      return;
    }

    isLoadingRef.current = true;
    setErrorMsg(null);

    readFile(filePath)
      .then((c) => {
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: c },
        });
        setBaselineContent(tabId, c);
        setContent(c);

        loadLanguage(filePath).then((lang) => {
          if (lang) {
            view.dispatch({
              effects: languageCompartment.current.reconfigure(lang),
            });
          }
        });
      })
      .catch((err) => {
        setErrorMsg(String(err));
      })
      .finally(() => {
        isLoadingRef.current = false;
      });
  }, [filePath, tabId, setBaselineContent]);

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
        <div className="flex h-7 shrink-0 items-center gap-1 border-b border-border/30 bg-card px-2">
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
      {/* Toolbar */}
      <div className="flex h-7 shrink-0 items-center gap-1 border-b border-border/30 bg-card px-2">
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

      {/* Rendered markdown view */}
      {isMd && viewMode === "rendered" && (
        <MarkdownRendered content={renderedContent} />
      )}

      {/* CodeMirror container — hidden when showing rendered view */}
      <div
        ref={containerRef}
        className="flex-1 min-h-0 overflow-hidden [&_.cm-editor]:h-full [&_.cm-scroller]:overflow-auto"
        style={{ display: isMd && viewMode === "rendered" ? "none" : undefined }}
      />
    </div>
  );
}
