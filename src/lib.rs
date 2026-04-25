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
//! ## Idiomatic one-liners
//!
//! On top of the libzim-mirror surface the crate provides an ergonomic
//! layer for common use cases:
//!
//! ```no_run
//! use zimru::Archive;
//! let a = Archive::open("wiki.zim")?;
//!
//! // Read an article as text in one call (follows redirects):
//! let html: String = a.get_text("home")?;
//!
//! // Metadata as UTF-8:
//! let title = a.metadata_str("Title")?;
//!
//! // Walk only the article (non-redirect) entries:
//! for e in a.articles() { let e = e?; println!("{}", e.path()); }
//!
//! // Prefix range iteration via binary search:
//! for e in a.by_prefix(b'C', "images/") { let e = e?; /* … */ }
//!
//! // Parallel cluster scan (rayon):
//! let per_cluster: Vec<u64> = a.par_clusters(|_i, c| -> u64 {
//!     (0..c.blob_count()).filter_map(|i| c.blob(i).ok())
//!         .map(|b| b.len() as u64).sum()
//! })?;
//! let total: u64 = per_cluster.into_iter().sum();
//! # Ok::<(), zimru::Error>(())
//! ```
//!
//! See [`Archive`] for the full list of ergonomic methods.
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
    Archive, Blob, Entry, EntryIter, Item, PrefixIter, Summary, NS_ARTICLES_LEGACY,
    NS_CONTENT_NEW, NS_INDEX, NS_METADATA, NS_WELLKNOWN,
};
pub use cluster::{Cluster, Compression};
pub use dirent::{ArticleEntry, Dirent, RedirectEntry};
pub use error::{Error, Result};
pub use header::Header;
pub use mime::MimeList;
