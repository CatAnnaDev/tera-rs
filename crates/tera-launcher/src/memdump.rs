#[cfg(not(windows))]
fn main() {
    eprintln!("tera-memdump only runs on Windows (inside the CrossOver bottle)");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_main::run() {
        eprintln!("tera-memdump: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod windows_main {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, MAX_PATH};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    type Result<T> = std::result::Result<T, String>;

    extern "system" {
        fn ReadProcessMemory(
            process: HANDLE,
            address: *const core::ffi::c_void,
            buffer: *mut core::ffi::c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
    }

    struct Options {
        pid: u32,
        name: Option<String>,
        out: Option<PathBuf>,
        executable_only: bool,
        min_size: usize,
        from: usize,
        to: usize,
    }

    fn parse() -> Options {
        let mut options = Options {
            pid: 0,
            name: None,
            out: None,
            executable_only: false,
            min_size: 4096,
            from: 0,
            to: usize::MAX,
        };
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--pid" => options.pid = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                "--name" => options.name = args.next(),
                "--out" => options.out = args.next().map(PathBuf::from),
                "--executable" => options.executable_only = true,
                "--from" => {
                    options.from = args
                        .next()
                        .and_then(|v| usize::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                        .unwrap_or(0)
                }
                "--to" => {
                    options.to = args
                        .next()
                        .and_then(|v| usize::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                        .unwrap_or(usize::MAX)
                }
                "--min-size" => {
                    options.min_size = args.next().and_then(|v| v.parse().ok()).unwrap_or(4096)
                }
                _ => {}
            }
        }
        options
    }

    fn find_process(name: &str) -> Result<u32> {
        let wanted = name.to_ascii_lowercase();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot as isize == -1 {
                return Err("CreateToolhelp32Snapshot failed".into());
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = None;
            let mut ok = Process32FirstW(snapshot, &mut entry);
            while ok != 0 {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(MAX_PATH as usize);
                let text = OsString::from_wide(&entry.szExeFile[..end])
                    .to_string_lossy()
                    .to_ascii_lowercase();
                if text == wanted {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                ok = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            found.ok_or_else(|| format!("no process called {name}"))
        }
    }

    fn protection(flags: u32) -> &'static str {
        match flags & 0xff {
            PAGE_EXECUTE => "--x",
            PAGE_EXECUTE_READ => "r-x",
            PAGE_EXECUTE_READWRITE => "rwx",
            PAGE_EXECUTE_WRITECOPY => "rcx",
            PAGE_NOACCESS => "---",
            0x02 => "r--",
            0x04 => "rw-",
            0x08 => "rc-",
            _ => "r??",
        }
    }

    fn executable(flags: u32) -> bool {
        matches!(
            flags & 0xff,
            PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        )
    }

    pub fn run() -> Result<()> {
        let options = parse();
        let pid = match (options.pid, &options.name) {
            (0, Some(name)) => find_process(name)?,
            (0, None) => find_process("TERA.exe")?,
            (pid, _) => pid,
        };
        println!("reading pid {pid}");

        let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if process.is_null() {
            return Err(format!(
                "OpenProcess({pid}) failed; the target may refuse to be opened"
            ));
        }

        if let Some(out) = &options.out {
            std::fs::create_dir_all(out).map_err(|error| error.to_string())?;
        }

        let mut address: usize = 0;
        let mut regions = 0usize;
        let mut bytes_read = 0usize;
        let mut refused = 0usize;
        loop {
            let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            let size = unsafe {
                VirtualQueryEx(
                    process,
                    address as *const _,
                    &mut info,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if size == 0 {
                break;
            }
            let base = info.BaseAddress as usize;
            let length = info.RegionSize;
            let committed = info.State == MEM_COMMIT;
            let guarded = info.Protect & PAGE_GUARD != 0;
            let readable = info.Protect & 0xff != PAGE_NOACCESS;
            let wanted = committed
                && readable
                && !guarded
                && length >= options.min_size
                && base >= options.from
                && base < options.to
                && (!options.executable_only || executable(info.Protect));
            if wanted {
                let mut buffer = vec![0u8; length];
                let mut got = 0usize;
                let ok = unsafe {
                    ReadProcessMemory(
                        process,
                        base as *const _,
                        buffer.as_mut_ptr() as *mut _,
                        length,
                        &mut got,
                    )
                };
                if ok != 0 && got > 0 {
                    buffer.truncate(got);
                    regions += 1;
                    bytes_read += got;
                    match &options.out {
                        Some(out) => {
                            let target =
                                out.join(format!("{base:016x}.{}.bin", protection(info.Protect)));
                            std::fs::write(target, &buffer).map_err(|error| error.to_string())?;
                        }
                        None => println!(
                            "  {base:#018x}  {got:>12} bytes  {}",
                            protection(info.Protect)
                        ),
                    }
                } else {
                    refused += 1;
                }
            }
            address = match base.checked_add(length.max(1)) {
                Some(next) => next,
                None => break,
            };
        }
        unsafe { CloseHandle(process) };
        println!("{regions} regions, {bytes_read} bytes read, {refused} refused");
        Ok(())
    }
}
