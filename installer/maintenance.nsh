!macro PortalMaintenance PREFIX
Function ${PREFIX}AcquireMaintenance
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Global\PortalLoginer.Maintenance.v1") p .r0'
  StrCpy $MaintenanceHandle $0
  ${If} $MaintenanceHandle == 0
    MessageBox MB_ICONSTOP "无法取得安装权限，请关闭其他安装或更新窗口后重试。"
    Abort
  ${EndIf}
  System::Call 'kernel32::WaitForSingleObject(p $MaintenanceHandle, i 0) i .r0'
  ${If} $0 != 0
  ${AndIf} $0 != 128
    MessageBox MB_ICONSTOP "另一个安装、卸载或更新正在进行，请关闭对应窗口后重试。"
    Abort
  ${EndIf}
FunctionEnd
Function ${PREFIX}ReleaseMaintenance
  ${If} $MaintenanceHandle != 0
    System::Call 'kernel32::ReleaseMutex(p $MaintenanceHandle)'
    System::Call 'kernel32::CloseHandle(p $MaintenanceHandle)'
    StrCpy $MaintenanceHandle 0
  ${EndIf}
FunctionEnd
!macroend
