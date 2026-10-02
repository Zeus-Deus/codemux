# Context race acceptance fixture

CI-only package using the public CodeMux SDK and CLI. Each composer action waits for
the harness to run “CI release pending appends” from the command palette,
then tries to append to the draft it was started from. The harness types,
switches projects, or closes or replaces the chat pane before releasing the
append. This establishes the ordering without racing a timer against native
UI operations. The disable command still waits four seconds because disabling
the add-on stops the fixture itself. Outcomes are shown in the fixture's own
panel. The harness starts actions through the composer menu, which waits for
the current draft's broker registration. Never publish it or add it to the catalog.
