//! Reel ids and the train/val split. A split is a property of the reel: it depends on
//! the reel id and the salt only, so every record of a reel lands in the same split
//! and adding other reels never moves it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::DatasetError;
use crate::sha256;

/// The salt when none is given. Changing it reshuffles every split.
pub const DEFAULT_SALT: &str = "reelctl-dataset-v1";

/// The default share of reels in `val`, in percent.
pub const DEFAULT_VAL_PERCENT: u32 = 10;

/// The most a split may put in `val`, in percent.
pub const MAX_VAL_PERCENT: u32 = 50;

/// The longest reel id.
pub const MAX_REEL_ID_LEN: usize = 96;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    Train,
    Val,
}

impl Split {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Val => "val",
        }
    }
}

/// A reel id is 1 to 96 characters of `A-Za-z0-9._-` and does not start with `.`. It
/// ends up in record ids and file names, so nothing else is allowed.
pub fn is_valid_reel_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_REEL_ID_LEN
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A number from 0 to 99 that depends on the salt and the reel id only: the first
/// 8 bytes of SHA-256(`salt`, a zero byte, `reel_id`) as a big-endian integer, modulo 100.
pub fn bucket(reel_id: &str, salt: &str) -> u64 {
    let mut hasher = sha256::Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(&[0]);
    hasher.update(reel_id.as_bytes());
    let digest = hasher.finish();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(first) % 100
}

/// `val` when the reel's bucket is below `val_percent`, else `train`.
pub fn split_for(reel_id: &str, salt: &str, val_percent: u32) -> Split {
    if bucket(reel_id, salt) < u64::from(val_percent) {
        Split::Val
    } else {
        Split::Train
    }
}

/// `val_percent` must be 0 to [`MAX_VAL_PERCENT`].
pub fn check_val_percent(val_percent: u32) -> Result<u32, DatasetError> {
    if val_percent > MAX_VAL_PERCENT {
        Err(DatasetError::ValPercent(val_percent))
    } else {
        Ok(val_percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Computed outside the program with `printf 'salt\0id' | shasum -a 256`, the first
    /// 8 bytes read as a number modulo 100.
    #[test]
    fn buckets_match_values_computed_outside_the_program() {
        assert_eq!(bucket("reel-a", DEFAULT_SALT), 28);
        assert_eq!(bucket("lecture", DEFAULT_SALT), 78);
        assert_eq!(bucket("draft12", DEFAULT_SALT), 37);
        assert_eq!(bucket("reel-a", "other"), 54);
    }

    #[test]
    fn a_reel_is_val_when_its_bucket_is_below_the_percentage() {
        // reel-a has bucket 28.
        assert_eq!(split_for("reel-a", DEFAULT_SALT, 29), Split::Val);
        assert_eq!(split_for("reel-a", DEFAULT_SALT, 28), Split::Train);
        assert_eq!(split_for("reel-a", DEFAULT_SALT, 27), Split::Train);
        assert_eq!(split_for("reel-a", DEFAULT_SALT, 0), Split::Train);
    }

    #[test]
    fn a_zero_percent_split_has_no_val_reel() {
        for n in 0..500 {
            assert_eq!(
                split_for(&format!("reel-{n}"), DEFAULT_SALT, 0),
                Split::Train
            );
        }
    }

    #[test]
    fn adding_reels_never_moves_an_existing_reel() {
        let before: Vec<Split> = (0..200)
            .map(|n| split_for(&format!("r{n}"), DEFAULT_SALT, 10))
            .collect();
        // Nothing about other reels is an input, so computing a reel after "adding"
        // many others gives the same answer.
        for n in 200..400 {
            let _ = split_for(&format!("r{n}"), DEFAULT_SALT, 10);
        }
        let after: Vec<Split> = (0..200)
            .map(|n| split_for(&format!("r{n}"), DEFAULT_SALT, 10))
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn a_new_salt_moves_some_reels() {
        let moved = (0..200)
            .filter(|n| {
                let id = format!("r{n}");
                split_for(&id, DEFAULT_SALT, 30) != split_for(&id, "another-salt", 30)
            })
            .count();
        assert!(moved > 20, "only {moved} of 200 moved");
    }

    #[test]
    fn the_val_share_is_near_the_percentage() {
        for percent in [10u32, 30, 50] {
            let val = (0..2_000)
                .filter(|n| split_for(&format!("reel-{n}"), DEFAULT_SALT, percent) == Split::Val)
                .count();
            let share = val as f64 / 2_000.0 * 100.0;
            assert!(
                (share - f64::from(percent)).abs() < 3.0,
                "{percent}% gave {share}%"
            );
        }
    }

    #[test]
    fn the_id_and_the_salt_are_separated_so_they_cannot_run_together() {
        // Without the zero byte these two would hash the same bytes.
        assert_ne!(bucket("b", "a"), bucket("", "ab"));
        assert_ne!(
            bucket("bc", "a"),
            bucket("c", "ab"),
            "the salt and the id are one string"
        );
    }

    #[test]
    fn reel_ids_are_file_name_safe() {
        for good in [
            "a",
            "KzAcl9_Xc88",
            "draft12",
            "a.v1",
            "x-y_z",
            &"a".repeat(96),
        ] {
            assert!(is_valid_reel_id(good), "{good}");
        }
        for bad in [
            "",
            ".hidden",
            "has space",
            "a/b",
            "a\\b",
            "..",
            "é",
            "a:b",
            &"a".repeat(97),
        ] {
            assert!(!is_valid_reel_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn val_percent_is_bounded() {
        assert_eq!(check_val_percent(0).ok(), Some(0));
        assert_eq!(check_val_percent(50).ok(), Some(50));
        assert!(check_val_percent(51).is_err());
    }
}
