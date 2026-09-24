# Context race acceptance fixture

CI-only package using the public CodeMux SDK and CLI. Each command waits four
seconds and then tries to append to the draft it was started from, while the
desktop acceptance harness types, switches projects, closes or replaces the
chat pane, or disables the add-on. Outcomes are shown in the fixture's own
panel. Never publish it or add it to the catalog.
