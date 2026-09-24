# Activation race acceptance fixture

CI-only package using the public CodeMux SDK and CLI. Its activation waits
briefly without using active JavaScript time, so the desktop acceptance harness
can remove it while its native host is still starting. Never publish it or add
it to the catalog.
