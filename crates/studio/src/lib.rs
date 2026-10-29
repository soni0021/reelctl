//! The core of `reelctl serve`: the content-addressed store, the SQLite
//! catalogue, checkpoints, the job queue and the command catalogue. It does not
//! decode video or call a model; jobs run `reelctl` as child processes. See
//! `docs/specs/studio-service.md`.

#![warn(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalogue;
pub mod error;
pub mod store;

pub use catalogue::{now_ms, Asset, Catalogue, ASSET_KINDS, SCHEMA_VERSION};
pub use error::{Result, StudioError};
pub use store::{hash_file, ObjectInfo, Store, TreeEntry, STUDIO_DIR, TREE_KIND};
