//! The icon inside `rustysalamander.exe` (Explorer, shortcuts, Alt+Tab
//! before the window sets its own): `res/rustysalamander.ico`, the program's
//! window icon (`sal_ui::icon::program`) at the sizes Windows asks for.
//! `build.rs` puts it into the exe. The test below keeps the file the same
//! as the window icon; after changing that icon, write the file again with
//! `UPDATE_EXE_ICON=1 cargo test -p sal-app exe_icon`.

use std::io::Write;
use std::path::PathBuf;

use eframe::egui;

/// The sizes in the file: the small and large icons at 100–250 % scaling,
/// and 256 for Explorer's large views.
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// An `.ico` with one PNG image per size (Windows reads PNG icons since
/// Vista).
fn ico() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|&s| (s, png(&sal_ui::icon::window_icon(sal_ui::icon::PROGRAM, sal_ui::icon::RUST, s)))).collect();
    let mut out = Vec::new();
    // ICONDIR: reserved, type 1 (icon), count
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        // ICONDIRENTRY: width and height (0 is 256), colors, reserved,
        // planes, bits per pixel, data size, data offset
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([side, side, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &images {
        out.extend(data);
    }
    out
}

/// An RGBA PNG.
fn png(icon: &egui::IconData) -> Vec<u8> {
    let (w, h) = (icon.width as usize, icon.height as usize);
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for row in icon.rgba.chunks_exact(w * 4) {
        raw.push(0); // no filter
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&raw).unwrap();
    let idat = z.finish().unwrap();

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        png.extend((data.len() as u32).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        png.extend(kind);
        png.extend(data);
        png.extend(crc.sum().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    // 8 bits, RGBA, deflate, no filter method, no interlace
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    png
}

#[test]
fn exe_icon_is_the_window_icon() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("res").join("rustysalamander.ico");
    let fresh = ico();
    if std::env::var_os("UPDATE_EXE_ICON").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &fresh).unwrap();
    }
    let saved = std::fs::read(&path).unwrap_or_default();
    assert!(saved == fresh, "{} is not the program's window icon: run UPDATE_EXE_ICON=1 cargo test -p sal-app exe_icon", path.display());
    // what Windows reads: the directory and the first image
    assert_eq!(&saved[..6], &[0, 0, 1, 0, SIZES.len() as u8, 0]);
    let first = u32::from_le_bytes(saved[18..22].try_into().unwrap()) as usize;
    assert_eq!(&saved[first..first + 8], b"\x89PNG\r\n\x1a\n");
}
