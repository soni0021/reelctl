//! The core of `reelctl serve`: the content-addressed store, the SQLite
//! catalogue, checkpoints, the job queue and the command catalogue. It does not
//! decode video or call a model; jobs run `reelctl` as child processes. See
//! `docs/specs/studio-service.md`.

#![warn(clippy::all)]
#![forbid(unsafe_code)]

pub mod catalogue;
pub mod checkpoint;
pub mod commands;
pub mod error;
pub mod jobs;
pub mod pipeline;
pub mod protocol;
pub mod queue;
pub mod service;
pub mod store;

pub use catalogue::{now_ms, Asset, Catalogue, ASSET_KINDS, SCHEMA_VERSION};
pub use checkpoint::{cache_key, Checkpoint, CheckpointDiff, Subject, SUBJECT_KINDS};
pub use error::{Result, StudioError};
pub use jobs::{Class, InputRef, Job, JobEvent, JobRequest, JobStatus};
pub use queue::{Limits, Notification, Queue, RunContext, RunOutcome, Runner};
pub use store::{hash_file, ObjectInfo, Store, TreeEntry, STUDIO_DIR, TREE_KIND};
