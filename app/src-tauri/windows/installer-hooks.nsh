; Hooks for the NSIS setup (bundle > windows > nsis > installerHooks in tauri.conf.json).
;
; On a real uninstall (not an update), Tauri's template already:
;   - removes the start-at-login entry, HKCU\...\CurrentVersion\Run\unflub
;   - offers "Delete the application data", which removes the settings in
;     %APPDATA%\ca.rmgn.unflub and the web view cache in %LOCALAPPDATA%\ca.rmgn.unflub
; This hook removes what the template leaves behind. Updates pass /UPDATE and keep everything.

; unflub reads install-source beside unflub.exe to know who updates it (src/install.rs). A package
; manager passes /SOURCE=<name> (winget: /SOURCE=winget); a plain setup writes "installer". An
; update (/UPDATE, no /SOURCE) keeps what is there.
!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  ${GetOptions} $CMDLINE "/SOURCE=" $R0
  ${IfNot} ${Errors}
    FileOpen $R1 "$INSTDIR\install-source" w
    FileWrite $R1 $R0
    FileClose $R1
  ${ElseIf} $UpdateMode <> 1
    FileOpen $R1 "$INSTDIR\install-source" w
    FileWrite $R1 "installer"
    FileClose $R1
  ${ElseIfNot} ${FileExists} "$INSTDIR\install-source"
    FileOpen $R1 "$INSTDIR\install-source" w
    FileWrite $R1 "installer"
    FileClose $R1
  ${EndIf}

  ; A restart after installing an update opens About once. Quit updates don't pass /R.
  ClearErrors
  ${GetOptions} $CMDLINE "/R" $R0
  ${If} $UpdateMode = 1
  ${AndIfNot} ${Errors}
    CreateDirectory "$LOCALAPPDATA\${BUNDLEID}"
    ClearErrors
    FileOpen $R1 "$LOCALAPPDATA\${BUNDLEID}\updated-version" w
    ${IfNot} ${Errors}
      FileWrite $R1 "${VERSION}"
      FileClose $R1
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Tauri's uninstaller removes only its own files, and a leftover file keeps the folder.
  ${If} $UpdateMode <> 1
    Delete "$INSTDIR\install-source"
  ${EndIf}
!macroend

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
