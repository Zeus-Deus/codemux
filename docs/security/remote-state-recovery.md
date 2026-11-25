# Recovering an unsafe remote state directory

`codemux-remote` stores its manifest, access secret, workspace registry, and worktrees beneath one state directory. On Linux the default is usually `~/.local/share/codemux-remote`; on macOS it is `~/Library/Application Support/codemux-remote`. An older installation may have created that directory with group or other write access.

If the helper reports that its default state directory was writable by other users, treat its contents as untrusted. Apply the same caution to a custom `--state-dir` that other users could access. **Do not chmod the old directory and restart the helper against it.** Another user may have altered the registry, manifest, or SQLite sidecar files before the check.

1. Stop `codemux-remote` and prevent its service from restarting while you recover. Close connections that use the old helper.
2. Move the old state directory out of the active path into a private quarantine directory on the same filesystem. Create the quarantine parent at mode `0700` first; a same-filesystem rename avoids copying a changing tree. Keep the old state isolated while you inspect it. If you cannot isolate it safely, leave the helper stopped and select a new private `--state-dir` instead.
3. Start the helper with a fresh state directory at mode `0700`. It can create a missing default directory itself. Reconnect the device so it uses the new manifest and access secret.
4. Inspect the quarantined data as untrusted input. Re-register only workspaces and paths you independently verify, using the supported Codemux UI or CLI. Do not copy the old `manifest.json`, `codemux.db`, `codemux.db-wal`, `codemux.db-shm`, or other secrets into the new state directory.

Moving the directory cannot undo any earlier access or changes, and a process with an already open file handle may still affect the old files. Keep the old state out of service throughout recovery. Rotate any other credentials that may have been exposed there.
