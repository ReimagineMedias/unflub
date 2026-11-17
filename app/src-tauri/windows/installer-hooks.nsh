; Hooks for the NSIS setup (bundle > windows > nsis > installerHooks in tauri.conf.json).
;
; On a real uninstall (not an update), Tauri's template already:
;   - removes the start-at-login entry, HKCU\...\CurrentVersion\Run\unflub
;   - offers "Delete the application data", which removes the settings in
;     %APPDATA%\ca.rmgn.unflub and the web view cache in %LOCALAPPDATA%\ca.rmgn.unflub
; This hook removes what the template leaves behind. Updates pass /UPDATE and keep everything.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; Task Manager and Settings > Apps > Startup keep their on/off switch here, beside the Run key.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"
    ; The install folder and installer language. Tauri only clears these with the app data, but
    ; nothing needs them once unflub is gone (an upgrade reads them before it uninstalls).
    DeleteRegKey HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
  ${EndIf}
!macroend
