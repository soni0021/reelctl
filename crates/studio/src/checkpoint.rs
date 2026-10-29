//! Subjects and their history. Every succeeded job appends one checkpoint (its
//! outputs, role to object hash) and moves its subject's head. Revert appends a
//! copy of an earlier checkpoint; branch starts a new subject from one. Nothing is
//! ever deleted or rewritten.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalogue::{now_ms, Catalogue};
use crate::error::{Result, StudioError};

/// The kinds of subject.
pub const SUBJECT_KINDS: [&str; 8] = [
    "reel",
    "project",
    "look",
    "presenter",
    "understanding",
    "dataset",
    "eval",
    "make_reels",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub parent_subject: Option<i64>,
    pub head_checkpoint: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: i64,
    pub subject: i64,
    pub parent: Option<i64>,
    pub job: Option<i64>,
    pub label: String,
    /// Output role to object hash.
    pub outputs: BTreeMap<String, String>,
    /// Made from an earlier job's outputs, with no child process.
    pub cached: bool,
    pub note: String,
    pub created_at: i64,
}

/// How two checkpoints differ, by output role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDiff {
    pub same: Vec<String>,
    pub changed: Vec<String>,
    pub only_in_a: Vec<String>,
    pub only_in_b: Vec<String>,
}

/// The cache key of a job: the hash of its command, its arguments in a normal
/// form (object keys sorted), its input hashes and the `reelctl` version. Two jobs
/// with the same key give the same outputs (the commands are deterministic).
pub fn cache_key(
    command: &str,
    args: &Value,
    inputs: &BTreeMap<String, String>,
    version: &str,
) -> String {
    let normal = serde_json::json!({
        "command": command,
        "args": sorted(args),
        "inputs": inputs,
        "reelctl": version,
    });
    blake3::hash(normal.to_string().as_bytes())
        .to_hex()
        .to_string()
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let ordered: BTreeMap<&String, Value> = map
                .iter()
                .map(|(key, value)| (key, sorted(value)))
                .collect();
            serde_json::to_value(ordered).unwrap_or(Value::Null)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn subject_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Subject> {
    Ok(Subject {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        parent_subject: row.get(3)?,
        head_checkpoint: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn checkpoint_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Checkpoint, String)> {
    Ok((
        Checkpoint {
            id: row.get(0)?,
            subject: row.get(1)?,
            parent: row.get(2)?,
            job: row.get(3)?,
            label: row.get(4)?,
            outputs: BTreeMap::new(),
            cached: row.get(6)?,
            note: row.get(7)?,
            created_at: row.get(8)?,
        },
        row.get::<_, String>(5)?,
    ))
}

fn with_outputs((mut checkpoint, outputs): (Checkpoint, String)) -> Result<Checkpoint> {
    checkpoint.outputs = serde_json::from_str(&outputs).map_err(|error| {
        StudioError::corrupt(format!(
            "checkpoint {} has unreadable outputs: {error}",
            checkpoint.id
        ))
    })?;
    Ok(checkpoint)
}

const CHECKPOINT_COLUMNS: &str =
    "id, subject, parent, job, label, outputs, cached, note, created_at";

impl Catalogue {
    pub fn create_subject(&self, kind: &str, name: &str) -> Result<Subject> {
        if !SUBJECT_KINDS.contains(&kind) {
            return Err(StudioError::invalid(format!(
                "subject kind \"{kind}\" is not one of {}",
                SUBJECT_KINDS.join(", ")
            )));
        }
        self.db.execute(
            "INSERT INTO subjects (kind, name, created_at) VALUES (?1, ?2, ?3)",
            params![kind, name, now_ms()],
        )?;
        let id = self.db.last_insert_rowid();
        self.subject(id)?
            .ok_or_else(|| StudioError::corrupt("a subject vanished as it was written"))
    }

    pub fn subject(&self, id: i64) -> Result<Option<Subject>> {
        Ok(self
            .db
            .query_row(
                "SELECT id, kind, name, parent_subject, head_checkpoint, created_at FROM subjects WHERE id = ?1",
                [id],
                subject_row,
            )
            .optional()?)
    }

    pub fn subjects(&self, kind: Option<&str>) -> Result<Vec<Subject>> {
        let mut statement = self.db.prepare(
            "SELECT id, kind, name, parent_subject, head_checkpoint, created_at FROM subjects
             WHERE ?1 IS NULL OR kind = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([kind], subject_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn require_subject(&self, id: i64) -> Result<Subject> {
        self.subject(id)?
            .ok_or_else(|| StudioError::invalid(format!("there is no subject {id}")))
    }

    /// Append a checkpoint to `subject` (its parent is the current head) and move
    /// the head to it, in one transaction. Every output must be a recorded object.
    pub fn append_checkpoint(
        &self,
        subject: i64,
        job: Option<i64>,
        label: &str,
        outputs: &BTreeMap<String, String>,
        cached: bool,
        note: &str,
    ) -> Result<Checkpoint> {
        self.require_subject(subject)?;
        for hash in outputs.values() {
            if self.object(hash)?.is_none() {
                return Err(StudioError::invalid(format!(
                    "output {hash} is not in the store"
                )));
            }
        }
        // The head is read inside the write transaction, so two jobs finishing
        // together on one subject still make one chain with the head at the last.
        let tx = rusqlite::Transaction::new_unchecked(
            &self.db,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let head: Option<i64> = tx.query_row(
            "SELECT head_checkpoint FROM subjects WHERE id = ?1",
            [subject],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO checkpoints (subject, parent, job, label, outputs, cached, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![subject, head, job, label, serde_json::to_string(outputs)?, cached, note, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE subjects SET head_checkpoint = ?1 WHERE id = ?2",
            params![id, subject],
        )?;
        tx.commit()?;
        self.checkpoint(id)?
            .ok_or_else(|| StudioError::corrupt("a checkpoint vanished as it was written"))
    }

    pub fn checkpoint(&self, id: i64) -> Result<Option<Checkpoint>> {
        self.db
            .query_row(
                &format!("SELECT {CHECKPOINT_COLUMNS} FROM checkpoints WHERE id = ?1"),
                [id],
                checkpoint_row,
            )
            .optional()?
            .map(with_outputs)
            .transpose()
    }

    /// A subject's checkpoints, oldest first.
    pub fn checkpoints(&self, subject: i64) -> Result<Vec<Checkpoint>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {CHECKPOINT_COLUMNS} FROM checkpoints WHERE subject = ?1 ORDER BY id"
        ))?;
        let rows = statement.query_map([subject], checkpoint_row)?;
        rows.map(|row| with_outputs(row?)).collect()
    }

    /// Make `subject`'s head an earlier state again: a new checkpoint whose
    /// outputs equal `target`'s, labelled `revert to #N`. History stays.
    pub fn revert(&self, subject: i64, target: i64) -> Result<Checkpoint> {
        let checkpoint = self
            .checkpoint(target)?
            .ok_or_else(|| StudioError::invalid(format!("there is no checkpoint {target}")))?;
        if checkpoint.subject != subject {
            return Err(StudioError::invalid(format!(
                "checkpoint {target} belongs to subject {}, not {subject}",
                checkpoint.subject
            )));
        }
        self.append_checkpoint(
            subject,
            None,
            &format!("revert to #{target}"),
            &checkpoint.outputs,
            false,
            "",
        )
    }

    /// A new subject that starts from checkpoint `from` (a copy, labelled
    /// `branch of #N`), for trying another path.
    pub fn branch(&self, from: i64, name: &str) -> Result<(Subject, Checkpoint)> {
        let source = self
            .checkpoint(from)?
            .ok_or_else(|| StudioError::invalid(format!("there is no checkpoint {from}")))?;
        let parent = self.require_subject(source.subject)?;
        let subject = self.create_subject(&parent.kind, name)?;
        self.db.execute(
            "UPDATE subjects SET parent_subject = ?1 WHERE id = ?2",
            params![parent.id, subject.id],
        )?;
        let checkpoint = self.append_checkpoint(
            subject.id,
            None,
            &format!("branch of #{from}"),
            &source.outputs,
            false,
            "",
        )?;
        Ok((self.require_subject(subject.id)?, checkpoint))
    }

    /// How checkpoints `a` and `b` differ, by output role.
    pub fn diff_checkpoints(&self, a: i64, b: i64) -> Result<CheckpointDiff> {
        let get = |id: i64| -> Result<Checkpoint> {
            self.checkpoint(id)?
                .ok_or_else(|| StudioError::invalid(format!("there is no checkpoint {id}")))
        };
        let (a, b) = (get(a)?, get(b)?);
        let mut diff = CheckpointDiff {
            same: Vec::new(),
            changed: Vec::new(),
            only_in_a: Vec::new(),
            only_in_b: Vec::new(),
        };
        for (role, hash) in &a.outputs {
            match b.outputs.get(role) {
                Some(other) if other == hash => diff.same.push(role.clone()),
                Some(_) => diff.changed.push(role.clone()),
                None => diff.only_in_a.push(role.clone()),
            }
        }
        diff.only_in_b = b
            .outputs
            .keys()
            .filter(|role| !a.outputs.contains_key(*role))
            .cloned()
            .collect();
        Ok(diff)
    }

    /// The outputs of the latest succeeded job with this cache key, if any (its
    /// checkpoint's), for a cache hit.
    pub fn cached_outputs(
        &self,
        cache_key: &str,
    ) -> Result<Option<(i64, BTreeMap<String, String>)>> {
        let found = self
            .db
            .query_row(
                "SELECT c.job, c.outputs FROM checkpoints c JOIN jobs j ON j.id = c.job
                 WHERE j.cache_key = ?1 AND j.status = 'succeeded' ORDER BY c.id DESC LIMIT 1",
                [cache_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        found
            .map(|(job, outputs)| Ok((job, serde_json::from_str(&outputs)?)))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ObjectInfo;
    use serde_json::json;

    fn object(catalogue: &Catalogue, byte: u8) -> String {
        let hash = format!("{:064x}", byte);
        catalogue
            .record_object(&ObjectInfo {
                hash: hash.clone(),
                size: 1,
                kind: "file".to_string(),
            })
            .unwrap();
        hash
    }

    fn outputs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(role, hash)| (role.to_string(), hash.to_string()))
            .collect()
    }

    #[test]
    fn checkpoints_chain_and_the_head_follows() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "talk").unwrap();
        assert_eq!(subject.head_checkpoint, None);
        let (a, b) = (object(&catalogue, 1), object(&catalogue, 2));
        let first = catalogue
            .append_checkpoint(
                subject.id,
                None,
                "init",
                &outputs(&[("out", &a)]),
                false,
                "",
            )
            .unwrap();
        let second = catalogue
            .append_checkpoint(
                subject.id,
                None,
                "cut-silence",
                &outputs(&[("out", &b)]),
                false,
                "",
            )
            .unwrap();
        assert_eq!(first.parent, None);
        assert_eq!(second.parent, Some(first.id));
        assert_eq!(
            catalogue
                .subject(subject.id)
                .unwrap()
                .unwrap()
                .head_checkpoint,
            Some(second.id)
        );
        assert_eq!(catalogue.checkpoints(subject.id).unwrap().len(), 2);
    }

    #[test]
    fn revert_appends_and_can_itself_be_reverted() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "talk").unwrap().id;
        let (a, b) = (object(&catalogue, 1), object(&catalogue, 2));
        let first = catalogue
            .append_checkpoint(subject, None, "init", &outputs(&[("out", &a)]), false, "")
            .unwrap();
        let second = catalogue
            .append_checkpoint(subject, None, "edit", &outputs(&[("out", &b)]), false, "")
            .unwrap();
        let back = catalogue.revert(subject, first.id).unwrap();
        assert_eq!(back.outputs, first.outputs);
        assert_eq!(back.label, format!("revert to #{}", first.id));
        assert_eq!(back.parent, Some(second.id));
        let forward = catalogue.revert(subject, second.id).unwrap();
        assert_eq!(forward.outputs, second.outputs);
        // Nothing was deleted.
        assert_eq!(catalogue.checkpoints(subject).unwrap().len(), 4);
        assert_eq!(
            catalogue.subject(subject).unwrap().unwrap().head_checkpoint,
            Some(forward.id)
        );
    }

    #[test]
    fn revert_to_another_subjects_checkpoint_is_refused() {
        let catalogue = Catalogue::in_memory().unwrap();
        let one = catalogue.create_subject("project", "one").unwrap().id;
        let two = catalogue.create_subject("project", "two").unwrap().id;
        let a = object(&catalogue, 1);
        let point = catalogue
            .append_checkpoint(one, None, "init", &outputs(&[("out", &a)]), false, "")
            .unwrap();
        assert!(catalogue.revert(two, point.id).is_err());
        assert!(catalogue.revert(one, 999).is_err());
    }

    #[test]
    fn a_branch_is_a_new_subject_that_starts_from_the_checkpoint() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "talk").unwrap().id;
        let a = object(&catalogue, 1);
        let point = catalogue
            .append_checkpoint(subject, None, "init", &outputs(&[("out", &a)]), false, "")
            .unwrap();
        let (branch, start) = catalogue.branch(point.id, "talk, punchier").unwrap();
        assert_eq!(branch.kind, "project");
        assert_eq!(branch.parent_subject, Some(subject));
        assert_eq!(branch.head_checkpoint, Some(start.id));
        assert_eq!(start.outputs, point.outputs);
        // The original subject is untouched.
        assert_eq!(
            catalogue.subject(subject).unwrap().unwrap().head_checkpoint,
            Some(point.id)
        );
    }

    #[test]
    fn outputs_must_be_stored_objects() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "talk").unwrap().id;
        let missing = outputs(&[("out", &"f".repeat(64))]);
        assert!(catalogue
            .append_checkpoint(subject, None, "x", &missing, false, "")
            .is_err());
        assert!(catalogue.create_subject("movie", "x").is_err());
    }

    #[test]
    fn checkpoints_compare_by_role() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("reel", "r").unwrap().id;
        let (a, b, c) = (
            object(&catalogue, 1),
            object(&catalogue, 2),
            object(&catalogue, 3),
        );
        let one = catalogue
            .append_checkpoint(
                subject,
                None,
                "1",
                &outputs(&[("dna", &a), ("views", &b)]),
                false,
                "",
            )
            .unwrap();
        let two = catalogue
            .append_checkpoint(
                subject,
                None,
                "2",
                &outputs(&[("dna", &a), ("views", &c), ("extra", &c)]),
                false,
                "",
            )
            .unwrap();
        let diff = catalogue.diff_checkpoints(one.id, two.id).unwrap();
        assert_eq!(diff.same, ["dna"]);
        assert_eq!(diff.changed, ["views"]);
        assert!(diff.only_in_a.is_empty());
        assert_eq!(diff.only_in_b, ["extra"]);
    }

    #[test]
    fn the_cache_key_ignores_key_order_and_sees_every_part() {
        let inputs = outputs(&[("input", &"a".repeat(64))]);
        let one = cache_key(
            "render",
            &json!({"size": "1080x1920", "backend": "ffmpeg"}),
            &inputs,
            "0.1.0",
        );
        let same = cache_key(
            "render",
            &json!({"backend": "ffmpeg", "size": "1080x1920"}),
            &inputs,
            "0.1.0",
        );
        assert_eq!(one, same);
        assert_ne!(
            one,
            cache_key(
                "render",
                &json!({"size": "540x960", "backend": "ffmpeg"}),
                &inputs,
                "0.1.0"
            )
        );
        assert_ne!(
            one,
            cache_key(
                "render",
                &json!({"size": "1080x1920", "backend": "ffmpeg"}),
                &inputs,
                "0.2.0"
            )
        );
        assert_ne!(
            one,
            cache_key(
                "qa",
                &json!({"size": "1080x1920", "backend": "ffmpeg"}),
                &inputs,
                "0.1.0"
            )
        );
        let other = outputs(&[("input", &"b".repeat(64))]);
        assert_ne!(
            one,
            cache_key(
                "render",
                &json!({"size": "1080x1920", "backend": "ffmpeg"}),
                &other,
                "0.1.0"
            )
        );
    }

    #[test]
    fn a_succeeded_jobs_outputs_are_found_by_its_cache_key() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "talk").unwrap().id;
        let a = object(&catalogue, 1);
        catalogue
            .db
            .execute(
                "INSERT INTO jobs (subject, command, args, inputs, class, priority, status, after, cache_key, env_names, created_at)
                 VALUES (?1, 'render', '{}', '{}', 'heavy', 0, 'succeeded', '[]', 'key-1', '[]', 0)",
                [subject],
            )
            .unwrap();
        let job = catalogue.db.last_insert_rowid();
        catalogue
            .append_checkpoint(
                subject,
                Some(job),
                "render",
                &outputs(&[("out", &a)]),
                false,
                "",
            )
            .unwrap();
        let (found_job, found) = catalogue.cached_outputs("key-1").unwrap().unwrap();
        assert_eq!(found_job, job);
        assert_eq!(found["out"], a);
        assert_eq!(catalogue.cached_outputs("key-2").unwrap(), None);
    }
}
