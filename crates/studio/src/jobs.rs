//! Jobs in the catalogue: what was asked, its state, its events. The queue
//! (`queue.rs`) moves them through their states; this module only reads and writes
//! rows, each change in one statement or transaction.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalogue::{now_ms, Catalogue};
use crate::error::{Result, StudioError};

/// A resource class: each has its own limit on jobs running at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Heavy,
    Light,
    Network,
}

impl Class {
    pub const ALL: [Class; 3] = [Class::Heavy, Class::Light, Class::Network];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heavy => "heavy",
            Self::Light => "light",
            Self::Network => "network",
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "heavy" => Ok(Self::Heavy),
            "light" => Ok(Self::Light),
            "network" => Ok(Self::Network),
            other => Err(StudioError::corrupt(format!(
                "unknown job class \"{other}\""
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Blocked,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Blocked => "blocked",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(match text {
            "queued" => Self::Queued,
            "blocked" => Self::Blocked,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            other => {
                return Err(StudioError::corrupt(format!(
                    "unknown job status \"{other}\""
                )))
            }
        })
    }

    /// A job in this state will not change again.
    pub fn finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

/// Where a job's input comes from: a stored object, or an output of an earlier
/// job (resolved when that job has succeeded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputRef {
    Hash(String),
    FromJob { from: i64, role: String },
}

/// What a producer asks for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRequest {
    pub subject: i64,
    pub command: String,
    pub args: Value,
    pub inputs: BTreeMap<String, InputRef>,
    pub class: Class,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub after: Vec<i64>,
    /// The names (never the values) of the environment variables the job gets.
    #[serde(default)]
    pub env_names: Vec<String>,
    /// Run even when an earlier job had the same cache key.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub subject: i64,
    pub command: String,
    pub args: Value,
    pub inputs: BTreeMap<String, InputRef>,
    pub class: Class,
    pub priority: i64,
    pub status: JobStatus,
    pub after: Vec<i64>,
    pub cache_key: String,
    pub exit_code: Option<i32>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub env_names: Vec<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub force: bool,
}

const JOB_COLUMNS: &str = "id, subject, command, args, inputs, class, priority, status, after, cache_key, exit_code, result, error, env_names, created_at, started_at, finished_at, force";
const JOB_COLUMN_COUNT: usize = 18;

fn job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<Option<String>>> {
    (0..JOB_COLUMN_COUNT)
        .map(|index| {
            let value: rusqlite::types::Value = row.get(index)?;
            Ok(match value {
                rusqlite::types::Value::Null | rusqlite::types::Value::Blob(_) => None,
                rusqlite::types::Value::Integer(n) => Some(n.to_string()),
                rusqlite::types::Value::Real(n) => Some(n.to_string()),
                rusqlite::types::Value::Text(t) => Some(t),
            })
        })
        .collect()
}

fn parse_job(cells: Vec<Option<String>>) -> Result<Job> {
    let need = |index: usize| -> Result<&str> {
        cells[index]
            .as_deref()
            .ok_or_else(|| StudioError::corrupt(format!("a job row has no column {index}")))
    };
    let int = |index: usize| -> Result<i64> {
        need(index)?
            .parse()
            .map_err(|_| StudioError::corrupt("a job row has a bad number"))
    };
    let opt_int = |index: usize| -> Result<Option<i64>> {
        cells[index]
            .as_deref()
            .map(|text| {
                text.parse()
                    .map_err(|_| StudioError::corrupt("a job row has a bad number"))
            })
            .transpose()
    };
    Ok(Job {
        id: int(0)?,
        subject: int(1)?,
        command: need(2)?.to_string(),
        args: serde_json::from_str(need(3)?)?,
        inputs: serde_json::from_str(need(4)?)?,
        class: Class::parse(need(5)?)?,
        priority: int(6)?,
        status: JobStatus::parse(need(7)?)?,
        after: serde_json::from_str(need(8)?)?,
        cache_key: need(9)?.to_string(),
        exit_code: opt_int(10)?.map(|code| code as i32),
        result: cells[11].as_deref().map(serde_json::from_str).transpose()?,
        error: cells[12].clone(),
        env_names: serde_json::from_str(need(13)?)?,
        created_at: int(14)?,
        started_at: opt_int(15)?,
        finished_at: opt_int(16)?,
        force: int(17)? != 0,
    })
}

/// One stored event of a job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobEvent {
    pub job: i64,
    pub seq: i64,
    pub ts: i64,
    pub event: Value,
}

impl Catalogue {
    /// Insert a job as `queued` (or `blocked` when it waits on unfinished jobs).
    pub fn insert_job(
        &self,
        request: &JobRequest,
        cache_key: &str,
        status: JobStatus,
    ) -> Result<Job> {
        self.db.execute(
            "INSERT INTO jobs (subject, command, args, inputs, class, priority, status, after, cache_key, env_names, created_at, force)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                request.subject,
                request.command,
                serde_json::to_string(&request.args)?,
                serde_json::to_string(&request.inputs)?,
                request.class.as_str(),
                request.priority,
                status.as_str(),
                serde_json::to_string(&request.after)?,
                cache_key,
                serde_json::to_string(&request.env_names)?,
                now_ms(),
                request.force,
            ],
        )?;
        self.require_job(self.db.last_insert_rowid())
    }

    pub fn job(&self, id: i64) -> Result<Option<Job>> {
        self.db
            .query_row(
                &format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"),
                [id],
                job_row,
            )
            .optional()?
            .map(parse_job)
            .transpose()
    }

    pub fn require_job(&self, id: i64) -> Result<Job> {
        self.job(id)?
            .ok_or_else(|| StudioError::invalid(format!("there is no job {id}")))
    }

    /// Jobs, newest first: of one subject and/or in one state, at most `limit`.
    pub fn jobs(
        &self,
        subject: Option<i64>,
        status: Option<JobStatus>,
        limit: i64,
    ) -> Result<Vec<Job>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE (?1 IS NULL OR subject = ?1) AND (?2 IS NULL OR status = ?2)
             ORDER BY id DESC LIMIT ?3"
        ))?;
        let rows = statement.query_map(
            params![subject, status.map(JobStatus::as_str), limit],
            job_row,
        )?;
        rows.map(|row| parse_job(row?)).collect()
    }

    /// Move a job to `status`, but only from one of `from`. Returns whether it moved.
    pub fn move_job(&self, id: i64, from: &[JobStatus], status: JobStatus) -> Result<bool> {
        let job = self.require_job(id)?;
        if !from.contains(&job.status) {
            return Ok(false);
        }
        let now = now_ms();
        let changed = self.db.execute(
            "UPDATE jobs SET status = ?1,
                 started_at = CASE WHEN ?1 = 'running' THEN ?2 ELSE started_at END,
                 finished_at = CASE WHEN ?1 IN ('succeeded','failed','cancelled','interrupted') THEN ?2 ELSE finished_at END
             WHERE id = ?3 AND status = ?4",
            params![status.as_str(), now, id, job.status.as_str()],
        )?;
        Ok(changed == 1)
    }

    /// Set the cache key once the job's inputs are known.
    pub fn set_cache_key(&self, id: i64, cache_key: &str) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET cache_key = ?1 WHERE id = ?2",
            params![cache_key, id],
        )?;
        Ok(())
    }

    /// Record how a finished job ended.
    pub fn finish_job(
        &self,
        id: i64,
        status: JobStatus,
        exit_code: Option<i32>,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET status = ?1, exit_code = ?2, result = ?3, error = ?4, finished_at = ?5 WHERE id = ?6",
            params![
                status.as_str(),
                exit_code,
                result.map(serde_json::to_string).transpose()?,
                error,
                now_ms(),
                id
            ],
        )?;
        Ok(())
    }

    /// On start: jobs that were running when the service stopped are interrupted
    /// (they can be retried); queued and blocked ones go on. Returns how many.
    pub fn interrupt_running(&self) -> Result<usize> {
        Ok(self.db.execute(
            "UPDATE jobs SET status = 'interrupted', finished_at = ?1, error = 'the service stopped while it ran'
             WHERE status = 'running'",
            [now_ms()],
        )?)
    }

    pub fn append_event(&self, job: i64, event: &Value) -> Result<i64> {
        let seq: i64 = self.db.query_row(
            "SELECT coalesce(max(seq), 0) + 1 FROM job_events WHERE job = ?1",
            [job],
            |row| row.get(0),
        )?;
        self.db.execute(
            "INSERT INTO job_events (job, seq, ts, event) VALUES (?1, ?2, ?3, ?4)",
            params![job, seq, now_ms(), serde_json::to_string(event)?],
        )?;
        Ok(seq)
    }

    /// A job's events after `since` (0 for all).
    pub fn events(&self, job: i64, since: i64) -> Result<Vec<JobEvent>> {
        let mut statement = self.db.prepare(
            "SELECT job, seq, ts, event FROM job_events WHERE job = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = statement.query_map(params![job, since], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (job, seq, ts, event) = row?;
            Ok(JobEvent {
                job,
                seq,
                ts,
                event: serde_json::from_str(&event)?,
            })
        })
        .collect()
    }

    /// The checkpoint a job made, if it made one.
    pub fn checkpoint_of_job(&self, job: i64) -> Result<Option<crate::checkpoint::Checkpoint>> {
        let id: Option<i64> = self
            .db
            .query_row(
                "SELECT id FROM checkpoints WHERE job = ?1 ORDER BY id LIMIT 1",
                [job],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| self.checkpoint(id))
            .transpose()
            .map(Option::flatten)
    }

    /// Jobs of `class` that can run now, best first: queued, or blocked with every
    /// `after` job succeeded (those are moved to queued). A blocked job whose
    /// `after` job failed, was cancelled or interrupted fails, with the reason.
    pub fn runnable(&self, class: Class) -> Result<Vec<Job>> {
        let mut statement = self.db.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE class = ?1 AND status IN ('queued', 'blocked')
             ORDER BY priority DESC, id ASC"
        ))?;
        let candidates: Vec<Job> = statement
            .query_map([class.as_str()], job_row)?
            .map(|row| parse_job(row?))
            .collect::<Result<_>>()?;
        let mut ready = Vec::new();
        'jobs: for job in candidates {
            for &before in &job.after {
                let earlier = self.require_job(before)?;
                match earlier.status {
                    JobStatus::Succeeded => {}
                    status if status.finished() => {
                        let reason = format!("after job {before} {}", status.as_str());
                        if self.move_job(
                            job.id,
                            &[JobStatus::Blocked, JobStatus::Queued],
                            JobStatus::Failed,
                        )? {
                            self.finish_job(job.id, JobStatus::Failed, None, None, Some(&reason))?;
                        }
                        continue 'jobs;
                    }
                    _ => continue 'jobs,
                }
            }
            if job.status == JobStatus::Blocked {
                self.move_job(job.id, &[JobStatus::Blocked], JobStatus::Queued)?;
            }
            ready.push(self.require_job(job.id)?);
        }
        Ok(ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(subject: i64, after: Vec<i64>) -> JobRequest {
        JobRequest {
            subject,
            command: "timeline.init".to_string(),
            args: json!({"x": 1}),
            inputs: BTreeMap::new(),
            class: Class::Light,
            priority: 0,
            after,
            env_names: vec!["PATH".to_string()],
            force: false,
        }
    }

    #[test]
    fn a_job_round_trips_and_moves_only_from_the_states_given() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "p").unwrap().id;
        let job = catalogue
            .insert_job(&request(subject, vec![]), "k", JobStatus::Queued)
            .unwrap();
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.env_names, ["PATH"]);
        assert!(!catalogue
            .move_job(job.id, &[JobStatus::Running], JobStatus::Succeeded)
            .unwrap());
        assert!(catalogue
            .move_job(job.id, &[JobStatus::Queued], JobStatus::Running)
            .unwrap());
        assert!(catalogue.require_job(job.id).unwrap().started_at.is_some());
        catalogue
            .finish_job(
                job.id,
                JobStatus::Succeeded,
                Some(0),
                Some(&json!({"frames": "3"})),
                None,
            )
            .unwrap();
        let done = catalogue.require_job(job.id).unwrap();
        assert_eq!(
            (done.status, done.exit_code),
            (JobStatus::Succeeded, Some(0))
        );
        assert_eq!(done.result, Some(json!({"frames": "3"})));
    }

    #[test]
    fn events_are_numbered_per_job_and_read_after_a_point() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "p").unwrap().id;
        let job = catalogue
            .insert_job(&request(subject, vec![]), "k", JobStatus::Queued)
            .unwrap()
            .id;
        for n in 0..3 {
            catalogue.append_event(job, &json!({"n": n})).unwrap();
        }
        assert_eq!(catalogue.events(job, 0).unwrap().len(), 3);
        let later = catalogue.events(job, 2).unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].event, json!({"n": 2}));
    }

    #[test]
    fn a_blocked_job_runs_after_its_earlier_job_and_fails_with_it() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "p").unwrap().id;
        let first = catalogue
            .insert_job(&request(subject, vec![]), "a", JobStatus::Queued)
            .unwrap()
            .id;
        let second = catalogue
            .insert_job(&request(subject, vec![first]), "b", JobStatus::Blocked)
            .unwrap()
            .id;
        let ids = |jobs: Vec<Job>| jobs.into_iter().map(|job| job.id).collect::<Vec<_>>();
        assert_eq!(ids(catalogue.runnable(Class::Light).unwrap()), [first]);
        catalogue
            .move_job(first, &[JobStatus::Queued], JobStatus::Running)
            .unwrap();
        catalogue
            .finish_job(first, JobStatus::Succeeded, Some(0), None, None)
            .unwrap();
        assert_eq!(ids(catalogue.runnable(Class::Light).unwrap()), [second]);
        assert_eq!(
            catalogue.require_job(second).unwrap().status,
            JobStatus::Queued
        );

        let third = catalogue
            .insert_job(&request(subject, vec![]), "c", JobStatus::Queued)
            .unwrap()
            .id;
        let fourth = catalogue
            .insert_job(&request(subject, vec![third]), "d", JobStatus::Blocked)
            .unwrap()
            .id;
        catalogue
            .move_job(third, &[JobStatus::Queued], JobStatus::Running)
            .unwrap();
        catalogue
            .finish_job(third, JobStatus::Failed, Some(1), None, Some("boom"))
            .unwrap();
        let runnable = ids(catalogue.runnable(Class::Light).unwrap());
        assert!(!runnable.contains(&fourth));
        let failed = catalogue.require_job(fourth).unwrap();
        assert_eq!(failed.status, JobStatus::Failed);
        assert_eq!(
            failed.error.as_deref(),
            Some(&*format!("after job {third} failed"))
        );
    }

    #[test]
    fn priority_then_age_decides_the_order() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "p").unwrap().id;
        let low = catalogue
            .insert_job(&request(subject, vec![]), "a", JobStatus::Queued)
            .unwrap()
            .id;
        let mut urgent = request(subject, vec![]);
        urgent.priority = 5;
        let high = catalogue
            .insert_job(&urgent, "b", JobStatus::Queued)
            .unwrap()
            .id;
        let later = catalogue
            .insert_job(&request(subject, vec![]), "c", JobStatus::Queued)
            .unwrap()
            .id;
        let order: Vec<i64> = catalogue
            .runnable(Class::Light)
            .unwrap()
            .into_iter()
            .map(|job| job.id)
            .collect();
        assert_eq!(order, [high, low, later]);
        assert!(catalogue.runnable(Class::Heavy).unwrap().is_empty());
    }

    #[test]
    fn running_jobs_become_interrupted_on_start() {
        let catalogue = Catalogue::in_memory().unwrap();
        let subject = catalogue.create_subject("project", "p").unwrap().id;
        let running = catalogue
            .insert_job(&request(subject, vec![]), "a", JobStatus::Queued)
            .unwrap()
            .id;
        let queued = catalogue
            .insert_job(&request(subject, vec![]), "b", JobStatus::Queued)
            .unwrap()
            .id;
        catalogue
            .move_job(running, &[JobStatus::Queued], JobStatus::Running)
            .unwrap();
        assert_eq!(catalogue.interrupt_running().unwrap(), 1);
        assert_eq!(
            catalogue.require_job(running).unwrap().status,
            JobStatus::Interrupted
        );
        assert_eq!(
            catalogue.require_job(queued).unwrap().status,
            JobStatus::Queued
        );
    }
}
