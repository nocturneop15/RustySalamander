//! Writing next to the program when its folder is protected (e.g. under
//! Program Files): the program runs itself once as administrator for just
//! that, and waits.

use std::io;

/// The argument that makes the program only mark its folder for
/// multi-user mode and exit (run as administrator by
/// [`write_multi_user_marker`]).
pub const MARK_MULTI_USER: &str = "--mark-multi-user";

/// Handle [`MARK_MULTI_USER`]: `Some(exit code)` when the program was
/// started for it.
pub fn run_helper(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some(MARK_MULTI_USER) {
        return None;
    }
    let ok = sal_core::config::app_dir().is_some_and(|d| sal_core::config::write_multi_user_marker(&d).is_ok());
    Some(if ok { 0 } else { 1 })
}

/// Mark the program's folder for multi-user mode as administrator (Windows
/// asks the user), waiting until it is done.
#[cfg(windows)]
pub fn write_multi_user_marker() -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let exe = std::env::current_exe()?;
    let file = wide(exe.as_os_str());
    let verb = wide("runas".as_ref());
    let params = wide(MARK_MULTI_USER.as_ref());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // fails when the user says no to Windows' question
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.message()))?;
    if info.hProcess.is_invalid() {
        return Err(io::Error::other("no administrator process"));
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "the program's folder could not be written as administrator"))
    }
}

#[cfg(not(windows))]
pub fn write_multi_user_marker() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::PermissionDenied, "the program's folder is read-only"))
}
