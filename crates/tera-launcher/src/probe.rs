#[cfg(not(windows))]
fn main() {
    eprintln!("tera-ipc-probe only runs on Windows (inside the CrossOver bottle)");
}

#[cfg(windows)]
fn main() {
    windows::run();
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, FindWindowExW, RegisterClassExW, SendMessageW,
        WM_COPYDATA, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
    };

    static SAVED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_COPYDATA {
            let data = &*(lparam as *const COPYDATASTRUCT);
            let payload =
                std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize);
            let text = if payload.len() >= 2 && payload[1] == 0 {
                let units: Vec<u16> = payload
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect();
                String::from_utf16_lossy(&units)
            } else {
                String::from_utf8_lossy(payload).to_string()
            };
                    println!(
                "   reply event {} ({} bytes): {}",
                data.dwData,
                data.cbData,
                text.replace('\n', " ")
            );
            let index = SAVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let target = format!(
                "C:\\users\\crossover\\Temp\\reply_{}_{}.bin",
                data.dwData, index
            );
            if std::fs::write(&target, payload).is_ok() {
                println!("   saved {target}");
            }
            return 1;
        }
        DefWindowProcW(window, message, wparam, lparam)
    }

    pub fn run() {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide("TERA_IPC_PROBE");
            let mut descriptor: WNDCLASSEXW = std::mem::zeroed();
            descriptor.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
            descriptor.lpfnWndProc = Some(procedure);
            descriptor.hInstance = instance;
            descriptor.lpszClassName = class.as_ptr();
            RegisterClassExW(&descriptor);
            let window = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("probe").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );

            let target_class = wide("LAUNCHER_CLASS");
            let mut found = 0usize;
            let mut current: HWND = std::ptr::null_mut();
            loop {
                current = FindWindowExW(
                    std::ptr::null_mut(),
                    current,
                    target_class.as_ptr(),
                    std::ptr::null(),
                );
                if current.is_null() {
                    break;
                }
                found += 1;
                println!("launcher window {found}: {current:?}");
                let listing = std::env::args().any(|value| value == "--list");
                if listing {
                    continue;
                }
                let events: Vec<usize> = std::env::args()
                    .skip_while(|value| value != "--events")
                    .nth(1)
                    .map(|value| {
                        value
                            .split(',')
                            .filter_map(|item| item.parse().ok())
                            .collect()
                    })
                    .unwrap_or_else(|| vec![1usize, 3, 5]);
                for event in events {
                    let payload: [u8; 4] = [0, 0, 0, 0];
                    let data = COPYDATASTRUCT {
                        dwData: event,
                        cbData: payload.len() as u32,
                        lpData: payload.as_ptr() as *mut _,
                    };
                    println!("-> event {event}");
                    SendMessageW(
                        current,
                        WM_COPYDATA,
                        window as WPARAM,
                        &data as *const _ as LPARAM,
                    );
                    let deadline = std::time::Instant::now()
                        + std::time::Duration::from_millis(2500);
                    let mut message: windows_sys::Win32::UI::WindowsAndMessaging::MSG =
                        std::mem::zeroed();
                    while std::time::Instant::now() < deadline {
                        while windows_sys::Win32::UI::WindowsAndMessaging::PeekMessageW(
                            &mut message,
                            std::ptr::null_mut(),
                            0,
                            0,
                            windows_sys::Win32::UI::WindowsAndMessaging::PM_REMOVE,
                        ) != 0
                        {
                            windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&message);
                            windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&message);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
            if found == 0 {
                println!("no LAUNCHER_CLASS window found");
            }
        }
    }
}
