# Fault isolation acceptance fixture

CI-only package using the public CodeMux SDK and CLI. Its commands deliberately
throw, loop forever, recurse, allocate beyond the heap budget, or enqueue
endless promises in its native add-on host. Never publish or add it to the catalog.
The desktop acceptance harness verifies quarantine and continued core and
Project Brief operation. No permission or runtime exception is granted.
