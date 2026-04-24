//! `zimru` — independent Rust re-implementation of the ZIM file-format reader.
//!
//! This crate is a clean-room implementation written from the publicly
//! documented [ZIM file format] specification. It shares no code with the
//! GPL-licensed C++ `libzim` library and is distributed under the MIT license.
//!
//! ## API compatibility
//!
//! The public API mirrors libzim's user-facing surface ([`Archive`],
//! [`Entry`], [`Item`], [`Blob`]) using snake_case as is idiomatic in Rust.
//! Method names map one-for-one to their C++ equivalents (`getEntryByPath` →
//! [`Archive::get_entry_by_path`], `getItem` → [`Entry::get_item`], etc.) so
//! switching a Rust libzim binding for `zimru` is largely a `use`-statement
//! change.
//!
//! ## Example
//!
//! ```no_run
//! use zimru::Archive;
//!
//! let archive = Archive::open("wikipedia_en_100.zim")?;
//! let entry = archive.main_entry()?;
//! let item = entry.get_item(true)?;
//! println!("{} ({} bytes)", item.path(), item.size()?);
//! # Ok::<(), zimru::Error>(())
//! ```
//!
//! [ZIM file format]: https://wiki.openzim.org/wiki/ZIM_file_format

pub mod archive;
pub mod cluster;
pub mod dirent;
pub mod error;
pub mod header;
pub mod mime;
mod raw;

pub use archive::{
    Archive, Blob, Entry, EntryIter, Item, NS_ARTICLES_LEGACY, NS_CONTENT_NEW, NS_INDEX,
    NS_METADATA, NS_WELLKNOWN,
};
pub use cluster::{Cluster, Compression};
pub use dirent::{ArticleEntry, Dirent, RedirectEntry};
pub use error::{Error, Result};
pub use header::Header;
pub use mime::MimeList;
