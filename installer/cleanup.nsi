Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!define VERSION "0.2.8"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PortalLoginer"
!include "cleanup.nsh"
Name "Portal Loginer 卸载与残留清理"
OutFile "..\dist\PortalLoginer-${VERSION}-Cleanup.exe"
InstallDir "$PROGRAMFILES64\PortalLoginer"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
Icon "..\assets\portal.ico"
!define MUI_WELCOMEPAGE_TITLE "卸载 Portal Loginer"
!define MUI_WELCOMEPAGE_TEXT "将彻底删除 Portal Loginer 的服务、程序、账号密码、配置、日志和启动入口。$\r$\n$\r$\n也可清理旧版卸载中断后留下的文件。$\r$\n$\r$\n请先退出设置、托盘和其他安装或卸载窗口，然后继续。"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "Portal Loginer 已卸载。现在可以重新安装。"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_LANGUAGE "SimpChinese"
Var MaintenanceHandle
!include "maintenance.nsh"
!insertmacro PortalMaintenance ""
Function .onGUIEnd
  Call ReleaseMaintenance
FunctionEnd
Function .onInit
  Call AcquireMaintenance
  SetRegView 64
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
FunctionEnd
Section "卸载与残留清理"
  !insertmacro PortalCleanup ""
  Call ReleaseMaintenance
SectionEnd
