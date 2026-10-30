//! Core of RustySalamander, a Rust rewrite of Open Salamander.
//!
//! Text model: every file is identified by a lossless [`std::path::PathBuf`]
//! (WTF-8 on Windows, so even names with unpaired UTF-16 surrogates round-trip),
//! while all display, sorting and matching works on UTF-8 [`String`]s. The
//! original C++ code used ANSI `char*` + `MAX_PATH` everywhere; this crate has
//! neither limitation.

pub mod config;
pub mod drives;
pub mod encoding;
pub mod entry;
pub mod fileops;
pub mod listing;
pub mod location;
pub mod masks;
pub mod quicksearch;
pub mod shell;
pub mod sort;

pub use entry::{EntryKind, FileEntry};
pub use listing::Listing;
pub use masks::MaskGroup;
pub use sort::{SortKey, SortSpec};
