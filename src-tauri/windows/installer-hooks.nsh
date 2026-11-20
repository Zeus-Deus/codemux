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
; exact (no wildcards) and a missing file costs nothing.
;
; An old sidecar can still be running during an update: the old app's updater
; (tauri-plugin-updater) starts this installer and calls `process::exit(0)`
; without Codemux's exit handler, and the template's running-app check only
; looks for codemux.exe. Its Claude sidecar exits once its stdin closes and
; its plugin host is killed with its job object, but neither is instant. So
; each file gets a few attempts over about 3.5 s. A file still in use after
; that is left in place and logged, and the install still succeeds; this
; hook runs on every install, so the next update deletes it. Nothing is
; scheduled for deletion at restart: NSIS reports a reboot deletion as
; scheduled even when an install without admin rights could not schedule
; it, and Windows deletes by path, so a rollback to v0.27.0 before the
; restart would lose its own sidecar.
;
; CI (scripts/windows-claude-smoke.mjs) seeds the three files and installs
; with `/S /UPDATE` while one is released during the retries and one stays
; locked throughout, then uninstalls, checking which files remain each time.
; To check a real upgrade by hand: install v0.27.0, run the new `*-setup.exe /UPDATE` (or update
; in-app), and confirm the install directory's `binaries\` holds only
; `codemux-sidecar-*.exe` and `node_modules\`; then uninstall and confirm
; `binaries\` is gone.

!include LogicLib.nsh

; Attempts per file, 500 ms apart.
!define CODEMUX_RETIRED_SIDECAR_ATTEMPTS 8

; Uses $R1 as the attempt counter; the caller saves it.
!macro CODEMUX_DELETE_RETIRED_SIDECAR FILE
  StrCpy $R1 0
  ${DoWhile} ${FileExists} "${FILE}"
    ${If} $R1 >= ${CODEMUX_RETIRED_SIDECAR_ATTEMPTS}
      DetailPrint "Codemux: ${FILE} is still in use; the next update will remove it."
      ${Break}
    ${EndIf}
    ${If} $R1 > 0
      Sleep 500
    ${EndIf}
    Delete "${FILE}"
    IntOp $R1 $R1 + 1
  ${Loop}
!macroend

!macro CODEMUX_DELETE_RETIRED_SIDECARS
  Push $R1
  ; scripts/windows-claude-smoke.mjs relies on this order: it takes the
  ; first file's deletion as the sign that the hook is retrying the second.
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECAR "$INSTDIR\binaries\codemux-claude-sidecar-x86_64-pc-windows-msvc.exe"
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECAR "$INSTDIR\binaries\codemux-managed-workflow-sidecar-x86_64-pc-windows-msvc.exe"
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECAR "$INSTDIR\binaries\codemux-plugin-host-x86_64-pc-windows-msvc.exe"
  Pop $R1
  ; A failed attempt sets the error flag; a template check must not see it.
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
