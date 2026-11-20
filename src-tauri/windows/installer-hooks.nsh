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
; each file gets a few attempts, then is scheduled for deletion at the next
; Windows restart (this needs rights to schedule it; an install without them
; leaves the file and logs it). That never asks for a restart: the template's
; finish page would offer one, selected, for a file nobody runs any more.
;
; CI (scripts/windows-claude-smoke.mjs) seeds the three files, installs with
; `/S /UPDATE` while one is locked until the hook is retrying it, uninstalls
; while one stays locked, and checks the outcome each time. To check a real
; upgrade by hand: install v0.27.0, run the new `*-setup.exe /UPDATE` (or update
; in-app), and confirm the install directory's `binaries\` holds only
; `codemux-sidecar-*.exe` and `node_modules\`; then uninstall and confirm
; `binaries\` is gone.

!include LogicLib.nsh

; Attempts before falling back to deletion at restart, 500 ms apart.
!define CODEMUX_RETIRED_SIDECAR_ATTEMPTS 8

; Uses $R1 as the attempt counter; the caller saves it.
!macro CODEMUX_DELETE_RETIRED_SIDECAR FILE
  StrCpy $R1 0
  ${DoWhile} ${FileExists} "${FILE}"
    ${If} $R1 >= ${CODEMUX_RETIRED_SIDECAR_ATTEMPTS}
      ; The flag is restored by the caller; here it reports whether this
      ; file was scheduled.
      SetRebootFlag false
      Delete /REBOOTOK "${FILE}"
      ${If} ${RebootFlag}
        DetailPrint "Codemux: ${FILE} is in use; Windows will delete it at the next restart."
      ${Else}
        DetailPrint "Codemux: could not delete ${FILE}; it is in use."
      ${EndIf}
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
  Push $R0
  Push $R1
  StrCpy $R0 0
  ${If} ${RebootFlag}
    StrCpy $R0 1
  ${EndIf}
  ; scripts/windows-claude-smoke.mjs locks the last file and releases it once
  ; the first two are gone; keep the plugin host last.
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECAR "$INSTDIR\binaries\codemux-claude-sidecar-x86_64-pc-windows-msvc.exe"
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECAR "$INSTDIR\binaries\codemux-managed-workflow-sidecar-x86_64-pc-windows-msvc.exe"
  !insertmacro CODEMUX_DELETE_RETIRED_SIDECAR "$INSTDIR\binaries\codemux-plugin-host-x86_64-pc-windows-msvc.exe"
  ; Restore the template's reboot flag, and clear the error flag the
  ; attempts set (a template check must not see it).
  ${If} $R0 = 1
    SetRebootFlag true
  ${Else}
    SetRebootFlag false
  ${EndIf}
  Pop $R1
  Pop $R0
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
