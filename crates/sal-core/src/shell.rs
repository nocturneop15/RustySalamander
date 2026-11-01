//! Shell integration: open with the associated program, edit, recycle bin.
//! All calls use the wide (UTF-16) Win32 APIs.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A NUL-terminated UTF-16 string for the Windows API.
#[cfg(windows)]
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Run `verb` on `file`, as Explorer would.
#[cfg(windows)]
fn shell_execute(verb: &str, file: &Path, params: Option<&str>, dir: Option<&Path>) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide(verb.as_ref());
    let file = wide(file.as_os_str());
    let params = params.map(|p| wide(p.as_ref()));
    let dir = dir.map(|d| wide(d.as_os_str()));
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // values <= 32 are errors
    if r as isize <= 32 {
        Err(io::Error::other(format!("ShellExecute failed (code {})", r as isize)))
    } else {
        Ok(())
    }
}

/// Open a file with its associated program (Enter in the panel).
pub fn open(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        shell_execute("open", path, None, path.parent())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

/// Open a file in an editor (F4): `editor` is a custom command line (see
/// [`open_with_program`]); `None` uses Notepad.
pub fn edit(path: &Path, editor: Option<&str>) -> io::Result<()> {
    match editor.filter(|e| !e.trim().is_empty()) {
        Some(cmd) => open_with_program(cmd, path),
        #[cfg(windows)]
        None => std::process::Command::new("notepad.exe").arg(path).spawn().map(|_| ()),
        #[cfg(not(windows))]
        None => {
            let ed = std::env::var("EDITOR").unwrap_or_else(|_| "xdg-open".into());
            std::process::Command::new(ed).arg(path).spawn().map(|_| ())
        }
    }
}

/// Program and argument string for running `command` on `file`: `{file}` in
/// the arguments is replaced by the quoted path; without it the quoted path
/// is appended. The program itself may be quoted (`"C:\Program Files\…"`).
pub fn program_command(command: &str, file: &Path) -> io::Result<(String, String)> {
    let (program, rest) = split_command(command).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no program is set"))?;
    let quoted = format!("\"{}\"", crate::encoding::display_path(file));
    let args = if rest.contains("{file}") {
        rest.replace("\"{file}\"", &quoted).replace("{file}", &quoted)
    } else if rest.is_empty() {
        quoted
    } else {
        format!("{rest} {quoted}")
    };
    Ok((program, args))
}

/// Run a user-chosen program (custom editor or viewer) on `file`.
pub fn open_with_program(command: &str, file: &Path) -> io::Result<()> {
    let (program, args) = program_command(command, file)?;
    let mut cmd = std::process::Command::new(&program);
    if let Some(dir) = file.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(&args);
    }
    #[cfg(not(windows))]
    cmd.args(args.split_whitespace().map(|a| a.trim_matches('"')));
    cmd.spawn().map(|_| ()).map_err(|e| io::Error::new(e.kind(), format!("cannot start \"{program}\": {e}")))
}

/// Which program "Open command prompt here" starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TerminalKind {
    /// The system's default terminal application (Windows Terminal unless
    /// the user picked the classic console host in Windows settings).
    #[default]
    Default,
    Cmd,
    /// PowerShell 7 (`pwsh.exe`) if installed, else Windows PowerShell.
    PowerShell,
    /// User command line; `{dir}` is replaced by the directory.
    Custom,
}

impl TerminalKind {
    pub const ALL: [TerminalKind; 4] = [TerminalKind::Default, TerminalKind::Cmd, TerminalKind::PowerShell, TerminalKind::Custom];

    /// The choice as Settings shows it (translated there).
    pub fn label(self) -> &'static str {
        match self {
            TerminalKind::Default => "Default terminal",
            TerminalKind::Cmd => "Command Prompt (cmd)",
            TerminalKind::PowerShell => "PowerShell",
            TerminalKind::Custom => "Custom command",
        }
    }
}

/// A resolved terminal launch: program, arguments, working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    /// Arguments quoted individually by the OS layer.
    pub args: Vec<String>,
    /// Argument string passed verbatim (custom commands keep the user's quoting).
    pub raw_args: Option<String>,
    pub dir: PathBuf,
}

/// Find an executable on PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Split a command line into program and the rest; the program may be quoted.
fn split_command(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        Some((rest[..end].to_owned(), rest[end + 1..].trim_start().to_owned()))
    } else {
        match s.find(char::is_whitespace) {
            Some(i) => Some((s[..i].to_owned(), s[i..].trim_start().to_owned())),
            None => Some((s.to_owned(), String::new())),
        }
    }
}

/// True if Windows is set to use the classic console host instead of a
/// terminal application (Settings > Privacy & security > For developers >
/// Terminal, stored in `HKCU\Console\%%Startup\DelegationTerminal`).
#[cfg(windows)]
fn default_terminal_is_conhost() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    const CONHOST: &str = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}";
    let key = wide("Console\\%%Startup".as_ref());
    let value = wide("DelegationTerminal".as_ref());
    let mut buf = [0u16; 64];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size) };
    if r != 0 {
        return false;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case(CONHOST)
}

/// Work out what to start for `kind` in `dir` without starting it.
pub fn terminal_launch(kind: TerminalKind, custom: &str, dir: &Path) -> io::Result<Launch> {
    let launch = |program: &str, args: Vec<String>| Launch { program: program.to_owned(), args, raw_args: None, dir: dir.to_path_buf() };
    let dir_str = crate::encoding::display_path(dir);
    match kind {
        TerminalKind::Custom => {
            let (program, rest) = split_command(custom).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no custom terminal command is set (Options > Terminal)"))?;
            let raw = rest.replace("{dir}", &dir_str);
            Ok(Launch { program: program.replace("{dir}", &dir_str), args: Vec::new(), raw_args: (!raw.is_empty()).then_some(raw), dir: dir.to_path_buf() })
        }
        #[cfg(windows)]
        TerminalKind::Cmd => Ok(launch("cmd.exe", Vec::new())),
        #[cfg(windows)]
        TerminalKind::PowerShell => Ok(launch(if which("pwsh.exe").is_some() { "pwsh.exe" } else { "powershell.exe" }, vec!["-NoLogo".into()])),
        #[cfg(windows)]
        TerminalKind::Default => {
            // a console program started normally opens in whatever terminal
            // Windows is configured to use; Windows Terminal additionally
            // needs -d to start in the right directory (';' separates wt commands)
            match which("wt.exe").filter(|_| !default_terminal_is_conhost()) {
                Some(_) => Ok(launch("wt.exe", vec!["-d".into(), dir_str.replace(';', "\\;")])),
                None => Ok(launch("cmd.exe", Vec::new())),
            }
        }
        #[cfg(not(windows))]
        TerminalKind::Cmd | TerminalKind::Default => Ok(launch("x-terminal-emulator", Vec::new())),
        #[cfg(not(windows))]
        TerminalKind::PowerShell => Ok(launch("pwsh", Vec::new())),
    }
}

/// Open a terminal in `dir` ("Open command prompt here").
pub fn open_terminal(kind: TerminalKind, custom: &str, dir: &Path) -> io::Result<()> {
    let l = terminal_launch(kind, custom, dir)?;
    let mut cmd = std::process::Command::new(&l.program);
    cmd.args(&l.args).current_dir(&l.dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        cmd.creation_flags(CREATE_NEW_CONSOLE);
        if let Some(raw) = &l.raw_args {
            cmd.raw_arg(raw);
        }
    }
    #[cfg(not(windows))]
    if let Some(raw) = &l.raw_args {
        cmd.args(raw.split_whitespace());
    }
    cmd.spawn().map(|_| ()).map_err(|e| io::Error::new(e.kind(), format!("cannot start \"{}\": {e}", l.program)))
}

/// Move items to the recycle bin.
#[cfg(windows)]
pub fn recycle(paths: &[PathBuf]) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    if paths.is_empty() {
        return Ok(());
    }
    // double-NUL-terminated list of NUL-terminated paths
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(wide(crate::listing::absolute(p).as_os_str()));
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as _,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 {
        Err(io::Error::other(format!("moving to the Recycle Bin failed (code {r:#x})")))
    } else if op.fAnyOperationsAborted != 0 {
        Err(io::Error::new(io::ErrorKind::Interrupted, "operation aborted"))
    } else {
        Ok(())
    }
}

/// Elsewhere there is no Recycle Bin.
#[cfg(not(windows))]
pub fn recycle(_paths: &[PathBuf]) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "recycle bin is not supported on this platform"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_command_parsing() {
        let dir = Path::new(r"C:\Data\složka 🦎");
        let l = terminal_launch(TerminalKind::Custom, r#""C:\Program Files\Alacritty\alacritty.exe" --working-directory "{dir}""#, dir).unwrap();
        assert_eq!(l.program, r"C:\Program Files\Alacritty\alacritty.exe");
        assert_eq!(l.raw_args.as_deref(), Some(r#"--working-directory "C:\Data\složka 🦎""#));
        let l = terminal_launch(TerminalKind::Custom, "nu.exe", dir).unwrap();
        assert_eq!((l.program.as_str(), l.raw_args), ("nu.exe", None));
        assert_eq!(l.dir, dir);
        assert!(terminal_launch(TerminalKind::Custom, "   ", dir).is_err());
    }

    #[test]
    fn program_command_lines() {
        let f = Path::new(r"C:\Data\zpráva 🦎.txt");
        let q = format!("\"{}\"", f.display());
        assert_eq!(program_command(r#""C:\Program Files\Notepad++\notepad++.exe""#, f).unwrap(), (r"C:\Program Files\Notepad++\notepad++.exe".to_owned(), q.clone()));
        assert_eq!(program_command("code.cmd --reuse-window", f).unwrap(), ("code.cmd".to_owned(), format!("--reuse-window {q}")));
        assert_eq!(program_command("viewer.exe /open {file} /readonly", f).unwrap().1, format!("/open {q} /readonly"));
        assert_eq!(program_command("viewer.exe \"{file}\"", f).unwrap().1, q, "already quoted placeholder");
        assert!(program_command("  ", f).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn builtin_terminals() {
        let dir = Path::new(r"C:\a;b");
        assert_eq!(terminal_launch(TerminalKind::Cmd, "", dir).unwrap().program, "cmd.exe");
        let ps = terminal_launch(TerminalKind::PowerShell, "", dir).unwrap();
        assert!(ps.program == "pwsh.exe" || ps.program == "powershell.exe");
        let d = terminal_launch(TerminalKind::Default, "", dir).unwrap();
        if d.program == "wt.exe" {
            assert_eq!(d.args, ["-d", r"C:\a\;b"]);
        } else {
            assert_eq!(d.program, "cmd.exe");
        }
    }
}
