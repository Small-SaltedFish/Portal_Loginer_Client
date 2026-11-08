!macro DeletePortalFile NAME
  IfFileExists "$INSTDIR\${NAME}" 0 +2
    Delete "$INSTDIR\${NAME}"
!macroend

!macro PortalCleanup PREFIX
${PREFIX}cleanup_retry:
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  ; The cleanup helper is embedded in this EXE, independent of installed files.
  File /oname=portal-uninstall-helper.exe "..\target\release\portal-service.exe"
  StrCpy $0 "launch_failed"
  ClearErrors
  ExecWait '"$PLUGINSDIR\portal-uninstall-helper.exe" uninstall-clean --directory "$INSTDIR"' $0
  ${If} $0 != 0
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "程序仍在运行或清理尚未完成。请退出设置和托盘后重试。未通过退出检查前不会删除配置和程序。" IDRETRY ${PREFIX}cleanup_retry
    SetErrorLevel 1
    Abort
  ${EndIf}
  SetOutPath "$TEMP"
  ClearErrors
  !insertmacro DeletePortalFile "portal-settings.exe"
  !insertmacro DeletePortalFile "portal-tray.exe"
  !insertmacro DeletePortalFile "portal-service.exe"
  !insertmacro DeletePortalFile "LICENSE"
  !insertmacro DeletePortalFile "NOTICE"
  !insertmacro DeletePortalFile "THIRD-PARTY-NOTICES.txt"
  ${If} ${Errors}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "仍有程序文件被占用，请关闭程序后重试。再次卸载会跳过已经删除的文件。" IDRETRY ${PREFIX}cleanup_retry
    SetErrorLevel 1
    Abort
  ${EndIf}
  SetShellVarContext all
  Delete "$DESKTOP\Portal Loginer.lnk"
  Delete "$SMPROGRAMS\Portal Loginer\Portal Loginer.lnk"
  Delete "$SMPROGRAMS\Portal Loginer\卸载.lnk"
  RMDir "$SMPROGRAMS\Portal Loginer"
  SetShellVarContext current
  Delete "$SMPROGRAMS\Portal Loginer.lnk"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PortalLoginer"
  DeleteRegKey HKCU "Software\SmallSaltedFish\PortalLoginer"
  ClearErrors
  !insertmacro DeletePortalFile "Uninstall.exe"
  ${If} ${Errors}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "旧卸载器仍在运行，请关闭其他卸载窗口后重试。" IDRETRY ${PREFIX}cleanup_retry
    SetErrorLevel 1
    Abort
  ${EndIf}
  ; Keep the registered entry and embedded uninstaller until the final step.
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
  RMDir "$INSTDIR"
  SetErrorLevel 0
!macroend
