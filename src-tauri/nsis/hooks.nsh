Var BrowserExtCheckbox
Var BrowserExtCheckboxState
Var EscapedInstDir
Var DetectedBrowserExe
Var DetectedBrowserArgs
Var OctoHeftLangCode

!macro NSIS_HOOK_LANGSTRINGS
  LangString BrowserExtCheckboxText ${LANG_ENGLISH} "Integrate and activate browser extension"
  LangString BrowserExtCheckboxText ${LANG_FRENCH} "Intégrer et activer l'extension navigateur"
  LangString BrowserExtCheckboxText ${LANG_SPANISH} "Integrar y activar la extensión del navegador"
  LangString BrowserExtCheckboxText ${LANG_GERMAN} "Browser-Erweiterung integrieren und aktivieren"
  LangString BrowserExtCheckboxText ${LANG_ARABIC} "دمج وتفعيل إضافة المتصفح"
  LangString BrowserExtCheckboxText ${LANG_RUSSIAN} "Интегрировать и активировать расширение браузера"
  LangString BrowserExtCheckboxText ${LANG_SIMPCHINESE} "集成并启用浏览器扩展程序"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Persist NSIS installer UI language so first launch matches the user's selected setup language
  StrCpy $OctoHeftLangCode "en"
  ${If} $LANGUAGE = ${LANG_FRENCH}
    StrCpy $OctoHeftLangCode "fr"
  ${ElseIf} $LANGUAGE = ${LANG_SPANISH}
    StrCpy $OctoHeftLangCode "es"
  ${ElseIf} $LANGUAGE = ${LANG_GERMAN}
    StrCpy $OctoHeftLangCode "de"
  ${ElseIf} $LANGUAGE = ${LANG_ARABIC}
    StrCpy $OctoHeftLangCode "ar"
  ${ElseIf} $LANGUAGE = ${LANG_RUSSIAN}
    StrCpy $OctoHeftLangCode "ru"
  ${ElseIf} $LANGUAGE = ${LANG_SIMPCHINESE}
    StrCpy $OctoHeftLangCode "zh"
  ${EndIf}

  WriteRegStr HKCU "Software\OctoHeft" "Language" "$OctoHeftLangCode"
  ${If} $UpdateMode <> 1
    WriteRegStr HKCU "Software\OctoHeft" "InstallerLanguageOverride" "$OctoHeftLangCode"
  ${EndIf}

  ; Seed AppData settings_state.json when absent so initial SettingsStore hydration uses $OctoHeftLangCode immediately
  SetShellVarContext current
  CreateDirectory "$APPDATA\octoheft"
  ${IfNot} ${FileExists} "$APPDATA\octoheft\settings_state.json"
    FileOpen $0 "$APPDATA\octoheft\settings_state.json" w
    FileWrite $0 '{"language":"$OctoHeftLangCode"}$\r$\n'
    FileClose $0
  ${EndIf}

  ; Register custom URI protocol handler: octoheft://download?url=...
  WriteRegStr HKCU "Software\Classes\octoheft" "" "URL:OctoHeft Protocol"
  WriteRegStr HKCU "Software\Classes\octoheft" "URL Protocol" ""
  WriteRegStr HKCU "Software\Classes\octoheft\DefaultIcon" "" "$INSTDIR\OctoHeft.exe,0"
  WriteRegStr HKCU "Software\Classes\octoheft\shell\open\command" "" '"$INSTDIR\OctoHeft.exe" "%1"'

  ; Register Explorer right-click context menu: Verify checksum with OctoHeft
  WriteRegStr HKCU "Software\Classes\*\shell\OctoHeftChecksum" "" "Verify checksum with OctoHeft"
  WriteRegStr HKCU "Software\Classes\*\shell\OctoHeftChecksum" "Icon" "$INSTDIR\OctoHeft.exe"
  WriteRegStr HKCU "Software\Classes\*\shell\OctoHeftChecksum\command" "" '"$INSTDIR\OctoHeft.exe" --verify "%1"'

  ; Generate Native Messaging Host manifests inside $INSTDIR\resources\ pointing directly to OctoHeft.exe
  ; (Chromium CreateProcess rejects CLI flags in "path"; OctoHeft.exe auto-handles both --native-host and browser origin args)
  CreateDirectory "$INSTDIR\resources"
  ${StrRep} $EscapedInstDir "$INSTDIR" "\" "\\"

  FileOpen $0 "$INSTDIR\resources\com.antigravity.octoheft.json" w
  FileWrite $0 '{$\r$\n'
  FileWrite $0 '  "name": "com.antigravity.octoheft",$\r$\n'
  FileWrite $0 '  "description": "OctoHeft Modern Download Accelerator Native Messaging Host",$\r$\n'
  FileWrite $0 '  "path": "$EscapedInstDir\\OctoHeft.exe",$\r$\n'
  FileWrite $0 '  "type": "stdio",$\r$\n'
  FileWrite $0 '  "allowed_origins": [$\r$\n'
  FileWrite $0 '    "chrome-extension://kalfiofhhhcdnnkjehidagebfikodflp/"$\r$\n'
  FileWrite $0 '  ]$\r$\n'
  FileWrite $0 '}$\r$\n'
  FileClose $0

  FileOpen $0 "$INSTDIR\resources\com.antigravity.octoheft.firefox.json" w
  FileWrite $0 '{$\r$\n'
  FileWrite $0 '  "name": "com.antigravity.octoheft",$\r$\n'
  FileWrite $0 '  "description": "OctoHeft Modern Download Accelerator Native Messaging Host",$\r$\n'
  FileWrite $0 '  "path": "$EscapedInstDir\\OctoHeft.exe",$\r$\n'
  FileWrite $0 '  "type": "stdio",$\r$\n'
  FileWrite $0 '  "allowed_extensions": [$\r$\n'
  FileWrite $0 '    "octoheft@antigravity.dev"$\r$\n'
  FileWrite $0 '  ]$\r$\n'
  FileWrite $0 '}$\r$\n'
  FileClose $0

  ; Register Native Messaging Host keys for Chrome, Edge, Brave, and Firefox
  WriteRegStr HKCU "Software\Google\Chrome\NativeMessagingHosts\com.antigravity.octoheft" "" "$INSTDIR\resources\com.antigravity.octoheft.json"
  WriteRegStr HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.antigravity.octoheft" "" "$INSTDIR\resources\com.antigravity.octoheft.json"
  WriteRegStr HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.antigravity.octoheft" "" "$INSTDIR\resources\com.antigravity.octoheft.json"
  WriteRegStr HKCU "Software\Mozilla\NativeMessagingHosts\com.antigravity.octoheft" "" "$INSTDIR\resources\com.antigravity.octoheft.firefox.json"
!macroend

!macro NSIS_HOOK_FINISH_SHOW
  ; Align checkbox with RTL layout when installer UI language is Arabic
  ${If} $(^RTL) = 1
    nsDialogs::SetRTL 1
  ${EndIf}
  ${NSD_CreateCheckbox} 120u 130u 195u 14u "$(BrowserExtCheckboxText)"
  Pop $BrowserExtCheckbox
  SetCtlColors $BrowserExtCheckbox "${MUI_TEXTCOLOR}" "${MUI_BGCOLOR}"
  SendMessage $BrowserExtCheckbox ${BM_SETCHECK} ${BST_CHECKED} 0
  !ifndef MUI_FORCECLASSICCONTROLS
  ${If} ${IsHighContrastModeActive}
  !endif
    System::Call 'UXTHEME::SetWindowTheme(p$BrowserExtCheckbox,w" ",w" ")'
  !ifndef MUI_FORCECLASSICCONTROLS
  ${EndIf}
  !endif
!macroend

!macro NSIS_HOOK_FINISH_LEAVE
  ${If} $PassiveMode <> 1
    ${NSD_GetState} $BrowserExtCheckbox $BrowserExtCheckboxState
    ${If} $BrowserExtCheckboxState = ${BST_CHECKED}
      StrCpy $DetectedBrowserExe ""
      StrCpy $DetectedBrowserArgs ""

      ; Prefer Chrome -> Edge -> Brave for one-click extensions page activation
      ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe" ""
      ${If} $R0 == ""
        ReadRegStr $R0 HKCU "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe" ""
      ${EndIf}
      ${If} $R0 != ""
      ${AndIf} ${FileExists} "$R0"
        StrCpy $DetectedBrowserExe "$R0"
        StrCpy $DetectedBrowserArgs "chrome://extensions"
      ${Else}
        ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\msedge.exe" ""
        ${If} $R0 != ""
        ${AndIf} ${FileExists} "$R0"
          StrCpy $DetectedBrowserExe "$R0"
          StrCpy $DetectedBrowserArgs "edge://extensions"
        ${Else}
          ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\brave.exe" ""
          ${If} $R0 != ""
          ${AndIf} ${FileExists} "$R0"
            StrCpy $DetectedBrowserExe "$R0"
            StrCpy $DetectedBrowserArgs "brave://extensions"
          ${EndIf}
        ${EndIf}
      ${EndIf}

      ${If} $DetectedBrowserExe != ""
        nsis_tauri_utils::RunAsUser "$DetectedBrowserExe" "$DetectedBrowserArgs"
      ${Else}
        ExecShell "open" "chrome://extensions"
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Cleanly remove all OctoHeft shell hooks, protocol schemes, startup keys, and browser integrations
  DeleteRegKey HKCU "Software\OctoHeft"
  DeleteRegKey HKCU "Software\Classes\octoheft"
  DeleteRegKey HKCU "Software\Classes\*\shell\OctoHeftChecksum"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "OctoHeft"

  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.antigravity.octoheft"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.antigravity.octoheft"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.antigravity.octoheft"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.antigravity.octoheft"

  Delete "$INSTDIR\resources\com.antigravity.octoheft.json"
  Delete "$INSTDIR\resources\com.antigravity.octoheft.firefox.json"
  RMDir /r "$INSTDIR\resources\extension"
  RMDir "$INSTDIR\resources"
  RMDir "$INSTDIR"

  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$APPDATA\octoheft"
    RMDir /r "$LOCALAPPDATA\OctoHeft"
  ${EndIf}
!macroend
