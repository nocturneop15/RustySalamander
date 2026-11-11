//! Text people (or what they paste) may type into any field, for the tests
//! that check every input: command injection, paths that leave the folder
//! or name devices, names Windows can't keep, control characters, odd
//! Unicode, format placeholders and very long text.

/// Every hostile input, short and long.
pub fn inputs() -> Vec<String> {
    let mut v: Vec<String> = SHORT.iter().map(|s| s.to_string()).collect();
    v.extend(long());
    v
}

/// Inputs that are short enough to type into a field in a test.
pub const SHORT: &[&str] = &[
    // empty and dots
    "",
    " ",
    "   \t ",
    ".",
    "..",
    "...",
    // leaving the folder
    "..\\escape",
    "../escape",
    "a\\..\\..\\..\\escape",
    "\\escape",
    "/escape",
    "\\\\",
    // devices and names Windows keeps
    "CON",
    "con.txt",
    "NUL",
    "nul.tar.gz",
    "Nul ",
    "COM1",
    "lpt9.log",
    "AUX",
    "PRN",
    "CONIN$",
    "COM¹",
    "\\\\.\\PhysicalDrive0",
    "\\\\.\\C:",
    "\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1",
    "\\??\\C:\\x",
    // names Windows changes or forbids
    "trailing.",
    "trailing ",
    "a<b",
    "a>b",
    "a|b",
    "a\"b",
    "a*b",
    "a?b",
    "a:b",
    "file.txt:stream",
    "x::$DATA",
    // control characters
    "\0",
    "a\0b",
    "bell\u{7}",
    "line\r\nbreak",
    "line\nbreak",
    "tab\there",
    "\u{1b}[31mred",
    "del\u{7f}",
    // command injection
    "& calc.exe",
    "| calc",
    "; calc",
    "$(calc)",
    "`calc`",
    "%COMSPEC%",
    "%PATH%",
    "^& calc",
    "\" & calc & \"",
    "x & start calc",
    "'; DROP TABLE tapes; --",
    "<script>alert(1)</script>",
    // placeholders of messages and formats
    "{error}",
    "{name}{name}{path}",
    "%s%s%s%n",
    "{0}{}",
    // Unicode
    "\u{202e}gpj.exe",
    "zero\u{200b}width",
    "e\u{301}\u{301}\u{301}\u{301}",
    "👨‍👩‍👧‍👦",
    "\u{feff}bom",
    "\u{fffd}\u{ffff}",
    "ｃａｌｃ．ｅｘｅ",
    "日本語のファイル",
    "עברית ملف",
    "Příliš žluťoučký kůň",
    // URLs and odd paths
    "file:///C:/Windows",
    "javascript:alert(1)",
    "ftp://user:pass@host.invalid/x",
    "x://",
    "://",
    "C:",
    "C:relative",
    "~",
    "~~",
];

/// Very long text: names past the limit, deep paths, huge masks.
pub fn long() -> Vec<String> {
    vec!["a".repeat(256), "é".repeat(300), "a".repeat(100_000), "a\\".repeat(2_000), "*?".repeat(10_000), "{error}".repeat(1_000), "\u{301}".repeat(5_000)]
}

/// Inputs no name or path may be accepted with: devices, forbidden
/// characters, control characters, leaving the folder, too long.
pub fn never_a_name() -> Vec<String> {
    let mut v: Vec<String> = [
        "", " ", ".", "..", "..\\escape", "a\\..\\..\\..\\escape", "CON", "con.txt", "NUL", "nul.tar.gz", "Nul ", "COM1", "lpt9.log", "AUX", "PRN", "CONIN$", "COM¹", "trailing.", "a<b", "a>b", "a|b",
        "a\"b", "a*b", "a?b", "a:b", "file.txt:stream", "\0", "a\0b", "bell\u{7}", "line\r\nbreak", "tab\there", "\u{1b}[31mred", "del\u{7f}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push("a".repeat(256));
    v
}
