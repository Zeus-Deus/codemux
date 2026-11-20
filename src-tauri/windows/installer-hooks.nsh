; NSIS installer hooks (tauri.conf.json > bundle > windows > nsis > installerHooks).
;
; Releases through v0.27.0 shipped three Bun sidecar executables as Tauri
; resources (~100 MB each). They are now one `codemux-sidecar-*.exe`, and
; nothing else removes the old files:
;   - The updater runs the new installer with /UPDATE, which never runs the
;     old uninstaller (tauri-cli 2.11.4 installer.nsi, PageLeaveReinstall), and
;     an installer only overwrites files, never removes them.
;   - The new uninstaller deletes only the new resource list, so the old files
;     would also survive a later uninstall and keep `binaries\` from being
;     removed.
; Delete them after every install and before every uninstall. Each path is
; exact (no wildcards) and a missing file is not an error.
;
; CI (scripts/windows-claude-smoke.mjs) seeds the three files, installs with
; `/S /UPDATE`, uninstalls, and checks they are gone each time. To check a real
; upgrade by hand: install v0.27.0, run the new `*-setup.exe /UPDATE` (or update
; in-app), and confirm the install directory's `binaries\` holds only
; `codemux-sidecar-*.exe` and `node_modules\`; then uninstall and confirm
; `binaries\` is gone.

!macro CODEMUX_DELETE_RETIRED_SIDECARS
  Delete "$INSTDIR\binaries\codemux-claude-sidecar-x86_64-pc-windows-msvc.exe"
  Delete "$INSTDIR\binaries\codemux-managed-workflow-sidecar-x86_64-pc-windows-msvc.exe"
  Delete "$INSTDIR\binaries\codemux-plugin-host-x86_64-pc-windows-msvc.exe"
  ; A missing file sets the error flag; leave the template's state as found.
  ClearErrors
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECARS
!macroend

; Before the template's own deletions, so its `RMDir "$INSTDIR\binaries"`
; finds the directory empty.
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECARS
!macroend
