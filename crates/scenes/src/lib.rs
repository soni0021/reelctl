//! Chart overlays drawn by `manimgx`, a separately installed program: the typed chart
//! spec, the fixed scene template (chart text is data in it, never code), and the
//! fail-closed runner. The OTIO track and the ffmpeg compositing are in `timeline`;
//! the command is in `reelctl`.

#![warn(clippy::all)]

mod error;
mod scene;
mod spec;
mod worker;

pub use error::{Result, SceneError};
pub use scene::{
    ascii_json, python_literal, scene_fps, scene_size, scene_source, MARKER, SCENE_NAME, TEMPLATE,
};
pub use spec::{
    axis_range, fnv1a, format_value, ChartKind, ChartSpec, Point, DEFAULT_WIDTH_PCT,
    MAX_LABEL_CHARS, MAX_SPEC_BYTES, MAX_TITLE_CHARS, MAX_UNIT_CHARS, MAX_VALUE,
};
pub use worker::{
    inspect_args, render_args, resolve_worker, Inspection, ManimgxCli, SceneJob, SceneRenderer,
    DEFAULT_TIMEOUT, INSTALL_HINT, WORKER_ENV,
};
