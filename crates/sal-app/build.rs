//! Puts the program's companions next to the executable (portable mode):
//! the language files (`lang/*.toml`) and the plugins, each in its own
//! folder under `plugins`. The plugins are separate DLLs, not part of the
//! program: they are built here with a cargo of their own.
//!
//! - The 7-Zip plugin (`plugins/sevenzip` in the source) goes to
//!   `plugins\7zip\7zip.spl` with `7z.dll`, 7-Zip's archive library, built
//!   from the 7-Zip source in `third_party` with MinGW (`mingw32-make`,
//!   gcc). Without a working toolchain the program is built anyway, without
//!   it.
//! - The SCP plugin (`plugins/scp`) goes to `plugins\scp\scp.spl`.
//! - The search plugin (`plugins/search`) goes to `plugins\search\search.spl`.

use std::path::{Path, PathBuf};
use std::process::Command;

const SEVEN_ZIP: &str = "../../third_party/7zip-26.05";

fn main() {
    println!("cargo:rerun-if-changed=lang");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // OUT_DIR is target/<profile>/build/sal-app-<hash>/out
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let Some(profile_dir) = out.ancestors().nth(3) else { return };

    let src = manifest.join("lang");
    let dst = profile_dir.join("lang");
    std::fs::create_dir_all(&dst).expect("create lang folder");
    for e in std::fs::read_dir(&src).expect("read lang folder").flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "toml") {
            println!("cargo:rerun-if-changed={}", p.display());
            std::fs::copy(&p, dst.join(e.file_name())).expect("copy language file");
        }
    }

    if let Err(e) = seven_zip_plugin(manifest, profile_dir) {
        println!("cargo:warning=the 7-Zip plugin was not built: {e}");
    }
    if let Err(e) = install_plugin(manifest, profile_dir, "sal-plugin-scp", "scp", "scp") {
        println!("cargo:warning=the SCP plugin was not built: {e}");
    }
    if let Err(e) = install_plugin(manifest, profile_dir, "sal-plugin-search", "search", "search") {
        println!("cargo:warning=the search plugin was not built: {e}");
    }
}

/// Build the plugin crate `package` (source in `plugins/<source>`) and put
/// it with its language files into `plugins\<folder>`; that folder.
fn install_plugin(manifest: &Path, profile_dir: &Path, package: &str, source: &str, folder: &str) -> Result<PathBuf, String> {
    let dll = build_plugin(manifest, profile_dir, package)?;
    let dir = profile_dir.join("plugins").join(folder);
    std::fs::create_dir_all(dir.join("lang")).map_err(|e| e.to_string())?;
    std::fs::copy(&dll, dir.join(format!("{folder}.spl"))).map_err(|e| format!("copy the plugin: {e}"))?;
    let lang = manifest.join("../../plugins").join(source).join("lang");
    for e in std::fs::read_dir(&lang).map_err(|e| format!("{}: {e}", lang.display()))?.flatten() {
        if e.path().extension().is_some_and(|x| x == "toml") {
            std::fs::copy(e.path(), dir.join("lang").join(e.file_name())).map_err(|e| format!("copy a language file: {e}"))?;
        }
    }
    Ok(dir)
}

/// `plugins\7zip`: the plugin DLL, its language files and `7z.dll`. Without
/// `7z.dll` the plugin is installed anyway and says what is missing.
fn seven_zip_plugin(manifest: &Path, profile_dir: &Path) -> Result<(), String> {
    let dir = install_plugin(manifest, profile_dir, "sal-plugin-7zip", "sevenzip", "7zip")?;
    let lib = seven_zip(&manifest.join(SEVEN_ZIP))?;
    println!("cargo:rerun-if-changed={}", lib.display());
    std::fs::copy(&lib, dir.join("7z.dll")).map_err(|e| format!("copy 7z.dll: {e}"))?;
    // 7-Zip's license goes with its library (LGPL, unRAR restriction)
    for doc in ["License.txt", "unRarLicense.txt"] {
        std::fs::copy(manifest.join(SEVEN_ZIP).join("DOC").join(doc), dir.join(doc)).map_err(|e| format!("copy {doc}: {e}"))?;
    }
    Ok(())
}

/// Build the plugin crate `package` (a cdylib in the workspace) with the
/// same profile, in a target folder of its own; its DLL.
fn build_plugin(manifest: &Path, profile_dir: &Path, package: &str) -> Result<PathBuf, String> {
    let root = manifest.join("../..");
    println!("cargo:rerun-if-changed={}", root.join("plugins").display());
    println!("cargo:rerun-if-changed={}", root.join("crates/sal-plugin-api").display());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target = profile_dir.parent().ok_or("no target folder")?.join("plugins-build");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(&root).args(["build", "-p", package, "--target-dir"]).arg(&target);
    if profile == "release" {
        cmd.arg("--release");
    }
    for var in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_MAKEFLAGS", "MAKEFLAGS", "MFLAGS"] {
        cmd.env_remove(var);
    }
    let output = cmd.output().map_err(|e| format!("cargo: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = err.lines().rev().take(15).collect();
        return Err(format!("cargo build -p {package} failed:\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
    }
    let dll = target.join(&profile).join(format!("{}.dll", package.replace('-', "_")));
    if dll.is_file() {
        Ok(dll)
    } else {
        Err(format!("{} not found", dll.display()))
    }
}

/// `7z.dll` (all formats, `Bundles/Format7zF`), built the first time; later
/// builds reuse it.
fn seven_zip(src: &Path) -> Result<PathBuf, String> {
    let bundle = src.join("CPP/7zip/Bundles/Format7zF");
    let dll = bundle.join("_o/7z.dll");
    if dll.is_file() {
        return Ok(dll);
    }
    if !bundle.join("makefile.gcc").is_file() {
        return Err(format!("7-Zip source not found at {}", src.display()));
    }
    let jobs = std::env::var("NUM_JOBS").unwrap_or_else(|_| "4".into());
    // without -Werror: newer gcc versions warn about code 7-Zip's own builds accept
    let status = Command::new("mingw32-make")
        .current_dir(&bundle)
        .args(["-f", "makefile.gcc", &format!("-j{jobs}"), "CC=gcc", "CXX=g++", "CFLAGS_WARN_WALL=-Wall -Wextra"])
        .status()
        .map_err(|e| format!("mingw32-make: {e}"))?;
    if status.success() && dll.is_file() {
        Ok(dll)
    } else {
        Err(format!("mingw32-make failed ({status})"))
    }
}
