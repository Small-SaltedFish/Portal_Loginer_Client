Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"
!ifndef VERSION
!define VERSION "0.2.8"
!endif
!define PRODUCT "Portal Loginer"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PortalLoginer"
Icon "..\assets\portal.ico"
UninstallIcon "..\assets\portal.ico"
!include "cleanup.nsh"
Name "${PRODUCT}"
OutFile "..\dist\PortalLoginer-${VERSION}-Setup.exe"
InstallDir "$PROGRAMFILES64\PortalLoginer"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUninstDetails show
VIProductVersion "0.2.8.0"
VIAddVersionKey /LANG=2052 "ProductName" "${PRODUCT}"
VIAddVersionKey /LANG=2052 "FileDescription" "Portal Loginer 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "Apache-2.0"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "安装校园网登录程序。$\r$\n$\r$\n安装后从桌面打开，输入账号密码并登录。关闭面板后，托盘和后台继续运行。$\r$\n$\r$\n升级前请退出旧版面板和托盘。"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE ValidateDirectory
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "安装完成。请双击桌面的 Portal Loginer 图标，按提示探测校园网络，验证账号后转入托盘。"
!insertmacro MUI_PAGE_FINISH
!define MUI_UNCONFIRMPAGE_TEXT_TOP "彻底移除程序、后台服务、账号密码、配置、日志和启动入口。$\r$\n$\r$\n请先从托盘退出程序。"
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
Var MaintenanceHandle
!include "maintenance.nsh"
!insertmacro PortalMaintenance ""
!insertmacro PortalMaintenance "un."
Function un.onInit
  Call un.AcquireMaintenance
FunctionEnd
Function .onGUIEnd
  Call ReleaseMaintenance
FunctionEnd
Function un.onGUIEnd
  Call un.ReleaseMaintenance
FunctionEnd
Var OwnerSid
Var HadPrevious

Function .onInit
  Call AcquireMaintenance
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "此版本需要 64 位 Windows。"
    Abort
  ${EndIf}
  SetRegView 64
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
  ${GetParameters} $0
  ${GetOptions} $0 "/OWNER_SID=" $OwnerSid
FunctionEnd

Function ValidateDirectory
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != ""
  ${AndIf} $0 != $INSTDIR
    MessageBox MB_ICONSTOP "升级时请使用原安装目录。如需迁移目录，请先卸载旧版。"
    Abort
  ${EndIf}
FunctionEnd

Function Rollback
  ${If} $HadPrevious == "yes"
    CopyFiles /SILENT "$PLUGINSDIR\previous\portal-service.exe" "$INSTDIR"
    IfFileExists "$PLUGINSDIR\previous\portal-settings.exe" 0 +2
      CopyFiles /SILENT "$PLUGINSDIR\previous\portal-settings.exe" "$INSTDIR"
    IfFileExists "$PLUGINSDIR\previous\portal-tray.exe" 0 +2
      CopyFiles /SILENT "$PLUGINSDIR\previous\portal-tray.exe" "$INSTDIR"
    ExecWait '"$INSTDIR\portal-service.exe" start' $0
  ${Else}
    IfFileExists "$INSTDIR\portal-service.exe" 0 +2
      ExecWait '"$INSTDIR\portal-service.exe" uninstall' $0
    Delete "$INSTDIR\portal-service.exe"
    Delete "$INSTDIR\portal-settings.exe"
    Delete "$INSTDIR\portal-tray.exe"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\NOTICE"
    Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
    SetOutPath "$PLUGINSDIR"
    RMDir "$INSTDIR"
  ${EndIf}
FunctionEnd

Section "安装"
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "..\target\release\portal-service.exe"
  ExecWait '"$PLUGINSDIR\portal-service.exe" prepare-install --directory "$INSTDIR"' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "无法准备安装目录或停止旧服务。请选择已有且受管理员保护的父目录，并退出旧版程序后重试。"
    Abort
  ${EndIf}
  StrCpy $HadPrevious "no"
  ${If} ${FileExists} "$INSTDIR\portal-service.exe"
    StrCpy $HadPrevious "yes"
    CreateDirectory "$PLUGINSDIR\previous"
    ClearErrors
    CopyFiles /SILENT "$INSTDIR\portal-service.exe" "$PLUGINSDIR\previous"
    IfFileExists "$INSTDIR\portal-settings.exe" 0 +2
      CopyFiles /SILENT "$INSTDIR\portal-settings.exe" "$PLUGINSDIR\previous"
    IfFileExists "$INSTDIR\portal-tray.exe" 0 +2
      CopyFiles /SILENT "$INSTDIR\portal-tray.exe" "$PLUGINSDIR\previous"
    ${If} ${Errors}
      ExecWait '"$INSTDIR\portal-service.exe" start' $0
      MessageBox MB_ICONSTOP "无法备份旧程序，安装已停止。"
      Abort
    ${EndIf}
  ${EndIf}
  SetOutPath "$INSTDIR"
  SetOverwrite try
  ClearErrors
  File "..\target\release\portal-service.exe"
  File "..\target\release\portal-settings.exe"
  File "..\target\release\portal-tray.exe"
  File "..\LICENSE"
  File "..\NOTICE"
  File "..\THIRD-PARTY-NOTICES.txt"
  ${If} ${Errors}
    Call Rollback
    MessageBox MB_ICONSTOP "程序文件正在使用。请退出旧版面板和托盘后重新安装。"
    Abort
  ${EndIf}
  ${If} $OwnerSid == ""
    ExecWait '"$INSTDIR\portal-service.exe" setup' $0
  ${Else}
    ExecWait '"$INSTDIR\portal-service.exe" setup --owner-sid $OwnerSid' $0
  ${EndIf}
  ${If} $0 != 0
    Call Rollback
    MessageBox MB_ICONSTOP "后台服务未能启动。请重新运行安装程序修复。"
    Abort
  ${EndIf}
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  SetShellVarContext all
  CreateShortcut "$DESKTOP\Portal Loginer.lnk" "$INSTDIR\portal-settings.exe"
  CreateDirectory "$SMPROGRAMS\Portal Loginer"
  CreateShortcut "$SMPROGRAMS\Portal Loginer\Portal Loginer.lnk" "$INSTDIR\portal-settings.exe"
  CreateShortcut "$SMPROGRAMS\Portal Loginer\卸载.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "Small-SaltedFish"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\portal-settings.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
  Call ReleaseMaintenance
SectionEnd

Section "Uninstall"
  SetRegView 64
  !insertmacro PortalCleanup "un."
  Call un.ReleaseMaintenance
SectionEnd
