#[cfg(windows)]
fn main() {
    use portal_desktop::platform::wide;
    use windows_sys::Win32::{
        Foundation::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    };
    if let Some(pid) = std::env::args().nth(1).and_then(|s| s.parse::<u32>().ok()) {
        unsafe extern "system" fn find(hwnd: HWND, data: LPARAM) -> i32 {
            let state = &mut *(data as *mut (u32, Vec<isize>));
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let mut name = [0u16; 128];
            let n = GetClassNameW(hwnd, name.as_mut_ptr(), 128);
            if pid == state.0
                && n > 0
                && String::from_utf16_lossy(&name[..n as usize]) == "tray_icon_app"
            {
                state.1.push(hwnd as isize);
            }
            1
        }
        let mut state = (pid, Vec::<isize>::new());
        unsafe {
            EnumWindows(Some(find), &mut state as *mut _ as isize);
        }
        let mut registered = 0;
        for hwnd in &state.1 {
            for id in 1..256 {
                let data = NOTIFYICONIDENTIFIER {
                    cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                    hWnd: *hwnd as HWND,
                    uID: id,
                    ..unsafe { std::mem::zeroed() }
                };
                let mut rect = unsafe { std::mem::zeroed() };
                if unsafe { Shell_NotifyIconGetRect(&data, &mut rect) } == 0 {
                    registered += 1;
                }
            }
        }
        println!(
            "pid={pid} tray_windows={} registered_icons={registered}",
            state.1.len()
        );
        assert_eq!(
            registered,
            std::env::args()
                .nth(2)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(1)
        );
        return;
    }
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            wide("STATIC").as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        let mut record = NOTIFYICONDATAW {
            hWnd: hwnd,
            uID: 29,
            uFlags: NIF_ICON | NIF_TIP,
            hIcon: LoadIconW(std::ptr::null_mut(), IDI_APPLICATION),
            ..std::mem::zeroed()
        };
        for (dst, src) in record
            .szTip
            .iter_mut()
            .zip(wide("Portal Loginer 初始化诊断"))
        {
            *dst = src;
        }
        SetLastError(0);
        let zero = Shell_NotifyIconW(NIM_ADD, &record);
        let zero_error = GetLastError();
        record.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        if zero != 0 {
            Shell_NotifyIconW(NIM_DELETE, &record);
        }
        SetLastError(0);
        let valid = Shell_NotifyIconW(NIM_ADD, &record);
        let valid_error = GetLastError();
        Shell_NotifyIconW(NIM_DELETE, &record);
        DestroyWindow(hwnd);
        println!("zero_size={zero} error={zero_error} correct_size={valid} error={valid_error}");
        assert_ne!(valid, 0);
    }
}
#[cfg(not(windows))]
fn main() {}
