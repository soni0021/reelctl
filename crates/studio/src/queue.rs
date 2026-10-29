//! The producer–consumer queue. Producers (`submit`) insert jobs. One dispatcher
//! thread starts each runnable job on its own thread while its class is under its
//! limit; the job's thread resolves its inputs, uses an earlier job's outputs when
//! the cache key matches, or runs it through the [`Runner`], stores the outputs,
//! appends the checkpoint and records how it ended. Every change is told to
//! subscribers. The runner is a trait, so the queue is tested without processes.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalogue::Catalogue;
use crate::checkpoint::cache_key;
use crate::error::{Result, StudioError};
use crate::jobs::{Class, InputRef, Job, JobRequest, JobStatus};
use crate::store::Store;

/// How often the dispatcher looks again without being woken.
const POLL: Duration = Duration::from_millis(250);

/// How many jobs of each class may run at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub heavy: usize,
    pub light: usize,
    pub network: usize,
}

impl Default for Limits {
    /// heavy 1 (a decode or a render already uses most of the cores), light 4,
    /// network 1.
    fn default() -> Self {
        Self {
            heavy: 1,
            light: 4,
            network: 1,
        }
    }
}

impl Limits {
    pub fn of(&self, class: Class) -> usize {
        match class {
            Class::Heavy => self.heavy,
            Class::Light => self.light,
            Class::Network => self.network,
        }
    }
}

/// What a running job is given.
pub struct RunContext<'a> {
    /// Input role to a path: a stored file, or a folder laid out from a tree.
    pub inputs: &'a BTreeMap<String, PathBuf>,
    /// The job's own empty folder; outputs are written here.
    pub scratch: &'a Path,
    /// Set when the job is cancelled: stop the child.
    pub cancel: &'a AtomicBool,
    /// Record an event of the job (a progress line, a warning).
    pub emit: &'a dyn Fn(Value),
}

/// How a run ended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOutcome {
    pub exit_code: Option<i32>,
    /// The result line's fields.
    pub result: Option<Value>,
    /// Output role to a file or folder in the scratch folder.
    pub outputs: BTreeMap<String, PathBuf>,
    pub error: Option<String>,
}

/// Runs one job: in the service, a child `reelctl`; in tests, a closure.
pub trait Runner: Send + Sync {
    fn run(&self, job: &Job, context: RunContext<'_>) -> RunOutcome;
}

/// What subscribers are told.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notification {
    JobStatus {
        job: i64,
        status: JobStatus,
    },
    JobEvent {
        job: i64,
        seq: i64,
        event: Value,
    },
    CheckpointCreated {
        subject: i64,
        checkpoint: i64,
        job: i64,
        cached: bool,
    },
}

struct State {
    limits: Limits,
    running: HashMap<Class, usize>,
    cancels: HashMap<i64, Arc<AtomicBool>>,
    subscribers: Vec<Sender<Notification>>,
    stop: bool,
    threads: Vec<JoinHandle<()>>,
}

struct Inner {
    store: Store,
    db_path: PathBuf,
    runner: Arc<dyn Runner>,
    version: String,
    state: Mutex<State>,
    wake: Condvar,
}

/// The queue of one workspace.
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

impl Inner {
    fn notify(&self, notification: Notification) {
        if let Ok(mut state) = self.state.lock() {
            state
                .subscribers
                .retain(|sender| sender.send(notification.clone()).is_ok());
        }
    }
}

impl Queue {
    /// Start the queue on `store` and the catalogue at `db_path`. Jobs that were
    /// running when the service last stopped become interrupted first.
    pub fn start(
        store: Store,
        db_path: &Path,
        limits: Limits,
        runner: Arc<dyn Runner>,
        version: &str,
    ) -> Result<Self> {
        let catalogue = Catalogue::open(db_path)?;
        catalogue.interrupt_running()?;
        let inner = Arc::new(Inner {
            store,
            db_path: db_path.to_path_buf(),
            runner,
            version: version.to_string(),
            state: Mutex::new(State {
                limits,
                running: HashMap::new(),
                cancels: HashMap::new(),
                subscribers: Vec::new(),
                stop: false,
                threads: Vec::new(),
            }),
            wake: Condvar::new(),
        });
        let dispatcher = {
            let inner = Arc::clone(&inner);
            std::thread::spawn(move || dispatch(inner, catalogue))
        };
        inner
            .state
            .lock()
            .map_err(|_| StudioError::corrupt("the queue lock is poisoned"))?
            .threads
            .push(dispatcher);
        Ok(Self { inner })
    }

    fn catalogue(&self) -> Result<Catalogue> {
        Catalogue::open(&self.inner.db_path)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.inner
            .state
            .lock()
            .map_err(|_| StudioError::corrupt("the queue lock is poisoned"))
    }

    /// Add a job (a producer). It is checked first: the subject must exist, a
    /// stored input must be in the store, and every `after` job must exist.
    pub fn submit(&self, request: JobRequest) -> Result<Job> {
        let catalogue = self.catalogue()?;
        catalogue.subject(request.subject)?.ok_or_else(|| {
            StudioError::invalid(format!("there is no subject {}", request.subject))
        })?;
        for (role, input) in &request.inputs {
            match input {
                InputRef::Hash(hash) => {
                    if !self.inner.store.has(hash) {
                        return Err(StudioError::invalid(format!(
                            "input {role}: {hash} is not in the store"
                        )));
                    }
                }
                InputRef::FromJob { from, .. } => {
                    catalogue.job(*from)?.ok_or_else(|| {
                        StudioError::invalid(format!("input {role}: there is no job {from}"))
                    })?;
                }
                InputRef::Asset { asset } => {
                    catalogue.asset(*asset)?.ok_or_else(|| {
                        StudioError::invalid(format!("input {role}: there is no asset {asset}"))
                    })?;
                }
            }
        }
        let mut waits = request.after.clone();
        waits.extend(request.inputs.values().filter_map(|input| match input {
            InputRef::FromJob { from, .. } => Some(*from),
            InputRef::Hash(_) | InputRef::Asset { .. } => None,
        }));
        waits.sort_unstable();
        waits.dedup();
        for &before in &waits {
            catalogue.require_job(before)?;
        }
        let request = JobRequest {
            after: waits.clone(),
            ..request
        };
        let status = if waits.is_empty() {
            JobStatus::Queued
        } else {
            JobStatus::Blocked
        };
        let job = catalogue.insert_job(&request, "", status)?;
        self.inner.notify(Notification::JobStatus {
            job: job.id,
            status,
        });
        self.inner.wake.notify_all();
        Ok(job)
    }

    /// Stop a job: a queued or blocked one at once, a running one through its
    /// runner (which ends its child). Returns the state it ends in, or is in.
    pub fn cancel(&self, job: i64) -> Result<JobStatus> {
        let catalogue = self.catalogue()?;
        if catalogue.move_job(
            job,
            &[JobStatus::Queued, JobStatus::Blocked],
            JobStatus::Cancelled,
        )? {
            self.inner.notify(Notification::JobStatus {
                job,
                status: JobStatus::Cancelled,
            });
            self.inner.wake.notify_all();
            return Ok(JobStatus::Cancelled);
        }
        if let Some(flag) = self.lock()?.cancels.get(&job) {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(catalogue.require_job(job)?.status)
    }

    /// A new job with the same request as `job` (which must be finished).
    pub fn retry(&self, job: i64) -> Result<Job> {
        let old = self.catalogue()?.require_job(job)?;
        if !old.status.finished() {
            return Err(StudioError::invalid(format!(
                "job {job} is {}; only a finished job is retried",
                old.status.as_str()
            )));
        }
        self.submit(JobRequest {
            subject: old.subject,
            command: old.command,
            args: old.args,
            inputs: old.inputs,
            class: old.class,
            priority: old.priority,
            after: old.after,
            env_names: old.env_names,
            force: old.force,
        })
    }

    pub fn limits(&self) -> Result<Limits> {
        Ok(self.lock()?.limits)
    }

    pub fn set_limits(&self, limits: Limits) -> Result<()> {
        if limits.heavy == 0 || limits.light == 0 || limits.network == 0 {
            return Err(StudioError::invalid(
                "every class needs a limit of at least 1",
            ));
        }
        self.lock()?.limits = limits;
        self.inner.wake.notify_all();
        Ok(())
    }

    /// Jobs running now, per class.
    pub fn running(&self) -> Result<HashMap<Class, usize>> {
        Ok(self.lock()?.running.clone())
    }

    /// Every later change to jobs and checkpoints.
    pub fn subscribe(&self) -> Result<Receiver<Notification>> {
        let (sender, receiver) = channel();
        self.lock()?.subscribers.push(sender);
        Ok(receiver)
    }

    /// Wait until no job is queued, blocked or running, or `timeout` passes.
    /// Returns whether it is idle.
    pub fn wait_idle(&self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let catalogue = self.catalogue()?;
            let busy = [JobStatus::Queued, JobStatus::Blocked, JobStatus::Running]
                .iter()
                .map(|status| {
                    catalogue
                        .jobs(None, Some(*status), 1)
                        .map(|jobs| jobs.len())
                })
                .sum::<Result<usize>>()?;
            if busy == 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stop starting jobs, cancel the running ones and wait for their threads.
    pub fn shutdown(&self) -> Result<()> {
        let threads = {
            let mut state = self.lock()?;
            state.stop = true;
            for flag in state.cancels.values() {
                flag.store(true, Ordering::SeqCst);
            }
            std::mem::take(&mut state.threads)
        };
        self.inner.wake.notify_all();
        for thread in threads {
            let _ = thread.join();
        }
        // Job threads started after the take above have finished by now: the
        // dispatcher, their only starter, has stopped.
        let rest = std::mem::take(&mut self.lock()?.threads);
        for thread in rest {
            let _ = thread.join();
        }
        Ok(())
    }
}

fn dispatch(inner: Arc<Inner>, catalogue: Catalogue) {
    loop {
        let Ok(mut state) = inner.state.lock() else {
            return;
        };
        if state.stop {
            return;
        }
        for class in Class::ALL {
            let limit = state.limits.of(class);
            let running = state.running.get(&class).copied().unwrap_or(0);
            if running >= limit {
                continue;
            }
            let Ok(ready) = catalogue.runnable(class) else {
                continue;
            };
            for job in ready.into_iter().take(limit - running) {
                match catalogue.move_job(job.id, &[JobStatus::Queued], JobStatus::Running) {
                    Ok(true) => {}
                    _ => continue,
                }
                let job_id = job.id;
                *state.running.entry(class).or_insert(0) += 1;
                let flag = Arc::new(AtomicBool::new(false));
                state.cancels.insert(job.id, Arc::clone(&flag));
                let worker = {
                    let inner = Arc::clone(&inner);
                    std::thread::spawn(move || {
                        let status = execute(&inner, &job, &flag).unwrap_or_else(|error| {
                            let message = error.to_string();
                            if let Ok(catalogue) = Catalogue::open(&inner.db_path) {
                                let _ = catalogue.finish_job(
                                    job.id,
                                    JobStatus::Failed,
                                    None,
                                    None,
                                    Some(&message),
                                );
                            }
                            JobStatus::Failed
                        });
                        inner.notify(Notification::JobStatus {
                            job: job.id,
                            status,
                        });
                        if let Ok(mut state) = inner.state.lock() {
                            if let Some(count) = state.running.get_mut(&job.class) {
                                *count = count.saturating_sub(1);
                            }
                            state.cancels.remove(&job.id);
                        }
                        inner.wake.notify_all();
                    })
                };
                state.threads.push(worker);
                // The lock is held here: tell subscribers through it.
                let running = Notification::JobStatus {
                    job: job_id,
                    status: JobStatus::Running,
                };
                state
                    .subscribers
                    .retain(|sender| sender.send(running.clone()).is_ok());
            }
        }
        state.threads.retain(|thread| !thread.is_finished());
        let _ = inner.wake.wait_timeout(state, POLL);
    }
}

/// Resolve an input to its hash, and for an asset the file it is read from.
fn resolve(
    catalogue: &Catalogue,
    store: &Store,
    role: &str,
    input: &InputRef,
) -> Result<(String, Option<PathBuf>)> {
    match input {
        InputRef::Hash(hash) => Ok((hash.clone(), None)),
        InputRef::Asset { asset } => {
            let asset = catalogue.asset(*asset)?.ok_or_else(|| {
                StudioError::invalid(format!("input {role}: there is no asset {asset}"))
            })?;
            // A copy in the store is the asset's bytes, wherever the source went.
            if asset.copied {
                if let Ok(path) = store.path(&asset.hash) {
                    if path.is_file() {
                        return Ok((asset.hash, Some(path)));
                    }
                }
            }
            if !catalogue.asset_unchanged(&asset)? {
                return Err(StudioError::invalid(format!(
                    "input {role}: {} changed or is gone since it was imported",
                    asset.source_path
                )));
            }
            Ok((asset.hash, Some(PathBuf::from(asset.source_path))))
        }
        InputRef::FromJob { from, role: output } => {
            let checkpoint = catalogue.checkpoint_of_job(*from)?.ok_or_else(|| {
                StudioError::invalid(format!("input {role}: job {from} made no checkpoint"))
            })?;
            let hash = checkpoint.outputs.get(output).cloned().ok_or_else(|| {
                StudioError::invalid(format!(
                    "input {role}: job {from} has no output \"{output}\""
                ))
            })?;
            Ok((hash, None))
        }
    }
}

/// Run one job to its end. Returns the state it ended in.
fn execute(inner: &Inner, job: &Job, cancel: &AtomicBool) -> Result<JobStatus> {
    let catalogue = Catalogue::open(&inner.db_path)?;
    let mut hashes = BTreeMap::new();
    let mut referenced = BTreeMap::new();
    for (role, input) in &job.inputs {
        let (hash, path) = resolve(&catalogue, &inner.store, role, input)?;
        if let Some(path) = path {
            referenced.insert(role.clone(), path);
        }
        hashes.insert(role.clone(), hash);
    }
    let key = cache_key(&job.command, &job.args, &hashes, &inner.version);
    catalogue.set_cache_key(job.id, &key)?;
    if !job.force {
        if let Some((earlier, outputs)) = catalogue.cached_outputs(&key)? {
            let checkpoint = catalogue.append_checkpoint(
                job.subject,
                Some(job.id),
                &job.command,
                &outputs,
                true,
                &format!("the outputs of job {earlier}, which had the same inputs"),
            )?;
            catalogue.finish_job(job.id, JobStatus::Succeeded, Some(0), None, None)?;
            inner.notify(Notification::CheckpointCreated {
                subject: job.subject,
                checkpoint: checkpoint.id,
                job: job.id,
                cached: true,
            });
            return Ok(JobStatus::Succeeded);
        }
    }

    let scratch = inner.store.scratch(job.id)?;
    let mut inputs = BTreeMap::new();
    for (role, hash) in &hashes {
        if let Some(path) = referenced.get(role) {
            inputs.insert(role.clone(), path.clone());
            continue;
        }
        let is_tree = catalogue
            .object(hash)?
            .is_some_and(|object| object.kind == "tree");
        let path = if is_tree {
            let dir = scratch.join("inputs").join(role);
            std::fs::create_dir_all(scratch.join("inputs"))
                .map_err(|error| StudioError::io(&scratch, error))?;
            inner.store.materialize(hash, &dir)?;
            dir
        } else {
            inner.store.path(hash)?
        };
        inputs.insert(role.clone(), path);
    }
    let emit = |event: Value| {
        if let Ok(catalogue) = Catalogue::open(&inner.db_path) {
            if let Ok(seq) = catalogue.append_event(job.id, &event) {
                inner.notify(Notification::JobEvent {
                    job: job.id,
                    seq,
                    event,
                });
            }
        }
    };
    let outcome = inner.runner.run(
        job,
        RunContext {
            inputs: &inputs,
            scratch: &scratch,
            cancel,
            emit: &emit,
        },
    );

    let status = if cancel.load(Ordering::SeqCst) {
        catalogue.finish_job(
            job.id,
            JobStatus::Cancelled,
            outcome.exit_code,
            outcome.result.as_ref(),
            Some("cancelled"),
        )?;
        JobStatus::Cancelled
    } else if outcome.exit_code == Some(0) {
        match store_outputs(inner, &catalogue, &outcome.outputs) {
            Ok(outputs) => {
                let checkpoint = catalogue.append_checkpoint(
                    job.subject,
                    Some(job.id),
                    &job.command,
                    &outputs,
                    false,
                    "",
                )?;
                catalogue.finish_job(
                    job.id,
                    JobStatus::Succeeded,
                    Some(0),
                    outcome.result.as_ref(),
                    None,
                )?;
                inner.notify(Notification::CheckpointCreated {
                    subject: job.subject,
                    checkpoint: checkpoint.id,
                    job: job.id,
                    cached: false,
                });
                JobStatus::Succeeded
            }
            Err(error) => {
                catalogue.finish_job(
                    job.id,
                    JobStatus::Failed,
                    Some(0),
                    outcome.result.as_ref(),
                    Some(&error.to_string()),
                )?;
                JobStatus::Failed
            }
        }
    } else {
        let error = outcome
            .error
            .clone()
            .unwrap_or_else(|| "the job failed".to_string());
        catalogue.finish_job(
            job.id,
            JobStatus::Failed,
            outcome.exit_code,
            outcome.result.as_ref(),
            Some(&error),
        )?;
        JobStatus::Failed
    };
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(status)
}

fn store_outputs(
    inner: &Inner,
    catalogue: &Catalogue,
    outputs: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, String>> {
    let mut stored = BTreeMap::new();
    for (role, path) in outputs {
        let info = if path.is_dir() {
            inner.store.put_tree(path)?
        } else if path.is_file() {
            inner.store.put_file(path)?
        } else {
            return Err(StudioError::invalid(format!(
                "output {role}: {} was not written",
                path.display()
            )));
        };
        record_tree(inner, catalogue, &info)?;
        stored.insert(role.clone(), info.hash);
    }
    Ok(stored)
}

/// Record an object, and every object a tree holds, in the catalogue.
fn record_tree(
    inner: &Inner,
    catalogue: &Catalogue,
    info: &crate::store::ObjectInfo,
) -> Result<()> {
    catalogue.record_object(info)?;
    if info.kind == "tree" {
        for entry in inner.store.tree(&info.hash)? {
            record_tree(
                inner,
                catalogue,
                &crate::store::ObjectInfo {
                    hash: entry.hash,
                    size: entry.size,
                    kind: entry.kind,
                },
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ObjectInfo;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    fn workspace(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("studio-queue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir).unwrap();
        let db = store.root().join("studio.db");
        (store, db)
    }

    /// Writes `out` = the command and its `x` argument, after `delay_ms` (polling
    /// the cancel flag), and counts how many run at once.
    struct Fake {
        now: AtomicUsize,
        most: AtomicUsize,
        runs: AtomicUsize,
    }

    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                now: AtomicUsize::new(0),
                most: AtomicUsize::new(0),
                runs: AtomicUsize::new(0),
            })
        }
    }

    impl Runner for Fake {
        fn run(&self, job: &Job, context: RunContext<'_>) -> RunOutcome {
            let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            self.runs.fetch_add(1, Ordering::SeqCst);
            (context.emit)(json!({"event": "stage", "name": job.command}));
            let delay = job.args["delay_ms"].as_u64().unwrap_or(0);
            let started = Instant::now();
            while started.elapsed() < Duration::from_millis(delay) {
                if context.cancel.load(Ordering::SeqCst) {
                    self.now.fetch_sub(1, Ordering::SeqCst);
                    return RunOutcome {
                        exit_code: Some(130),
                        ..RunOutcome::default()
                    };
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let fail = job.args["fail"].as_bool().unwrap_or(false);
            let input = context
                .inputs
                .get("in")
                .map(|path| std::fs::read_to_string(path).unwrap())
                .unwrap_or_default();
            let out = context.scratch.join("out.txt");
            std::fs::write(&out, format!("{}:{}:{input}", job.command, job.args["x"])).unwrap();
            self.now.fetch_sub(1, Ordering::SeqCst);
            if fail {
                return RunOutcome {
                    exit_code: Some(2),
                    error: Some("refused".to_string()),
                    ..RunOutcome::default()
                };
            }
            RunOutcome {
                exit_code: Some(0),
                result: Some(json!({"x": job.args["x"]})),
                outputs: BTreeMap::from([("out".to_string(), out)]),
                error: None,
            }
        }
    }

    fn request(subject: i64, command: &str, class: Class, args: Value) -> JobRequest {
        JobRequest {
            subject,
            command: command.to_string(),
            args,
            inputs: BTreeMap::new(),
            class,
            priority: 0,
            after: Vec::new(),
            env_names: Vec::new(),
            force: false,
        }
    }

    fn start(name: &str, runner: Arc<dyn Runner>) -> (Queue, Store, PathBuf, i64) {
        let (store, db) = workspace(name);
        let subject = Catalogue::open(&db)
            .unwrap()
            .create_subject("project", "p")
            .unwrap()
            .id;
        let queue = Queue::start(store.clone(), &db, Limits::default(), runner, "test").unwrap();
        (queue, store, db, subject)
    }

    #[test]
    fn a_job_runs_and_its_output_becomes_a_checkpoint() {
        let fake = Fake::new();
        let (queue, store, db, subject) = start("one", fake.clone());
        let events = queue.subscribe().unwrap();
        let job = queue
            .submit(request(subject, "render", Class::Heavy, json!({"x": 1})))
            .unwrap();
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        let catalogue = Catalogue::open(&db).unwrap();
        let done = catalogue.require_job(job.id).unwrap();
        assert_eq!(done.status, JobStatus::Succeeded, "{:?}", done.error);
        assert_eq!(done.result, Some(json!({"x": 1})));
        let checkpoint = catalogue.checkpoint_of_job(job.id).unwrap().unwrap();
        assert_eq!(
            store.read(&checkpoint.outputs["out"]).unwrap(),
            b"render:1:"
        );
        assert_eq!(
            catalogue.subject(subject).unwrap().unwrap().head_checkpoint,
            Some(checkpoint.id)
        );
        assert_eq!(catalogue.events(job.id, 0).unwrap().len(), 1);
        // The scratch folder is gone.
        assert!(!store.root().join("jobs").join(job.id.to_string()).exists());
        let seen: Vec<Notification> = events.try_iter().collect();
        assert!(seen
            .iter()
            .any(|n| matches!(n, Notification::CheckpointCreated { cached: false, .. })));
        assert!(seen.iter().any(|n| matches!(
            n,
            Notification::JobStatus {
                status: JobStatus::Succeeded,
                ..
            }
        )));
        queue.shutdown().unwrap();
    }

    #[test]
    fn the_class_limits_are_never_exceeded() {
        let fake = Fake::new();
        let (queue, _, _, subject) = start("limits", fake.clone());
        for x in 0..4 {
            queue
                .submit(request(
                    subject,
                    "render",
                    Class::Heavy,
                    json!({"x": x, "delay_ms": 60}),
                ))
                .unwrap();
        }
        assert!(queue.wait_idle(Duration::from_secs(20)).unwrap());
        assert_eq!(fake.most.load(Ordering::SeqCst), 1);
        // Four light jobs may run together.
        fake.most.store(0, Ordering::SeqCst);
        for x in 0..4 {
            queue
                .submit(request(
                    subject,
                    "cut",
                    Class::Light,
                    json!({"x": x, "delay_ms": 300}),
                ))
                .unwrap();
        }
        assert!(queue.wait_idle(Duration::from_secs(20)).unwrap());
        assert!(
            fake.most.load(Ordering::SeqCst) >= 2,
            "{}",
            fake.most.load(Ordering::SeqCst)
        );
        assert!(fake.most.load(Ordering::SeqCst) <= 4);
        queue.shutdown().unwrap();
    }

    #[test]
    fn a_chain_passes_outputs_along_and_a_failure_stops_it() {
        let fake = Fake::new();
        let (queue, store, db, subject) = start("chain", fake.clone());
        let first = queue
            .submit(request(subject, "init", Class::Light, json!({"x": 1})))
            .unwrap();
        let mut second = request(subject, "cut", Class::Light, json!({"x": 2}));
        second.inputs.insert(
            "in".to_string(),
            InputRef::FromJob {
                from: first.id,
                role: "out".to_string(),
            },
        );
        let second = queue.submit(second).unwrap();
        assert_eq!(second.status, JobStatus::Blocked);
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        let catalogue = Catalogue::open(&db).unwrap();
        let out = catalogue
            .checkpoint_of_job(second.id)
            .unwrap()
            .unwrap()
            .outputs["out"]
            .clone();
        assert_eq!(store.read(&out).unwrap(), b"cut:2:init:1:");

        let bad = queue
            .submit(request(
                subject,
                "init",
                Class::Light,
                json!({"x": 3, "fail": true}),
            ))
            .unwrap();
        let mut after = request(subject, "cut", Class::Light, json!({"x": 4}));
        after.after = vec![bad.id];
        let after = queue.submit(after).unwrap();
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        let bad = catalogue.require_job(bad.id).unwrap();
        assert_eq!((bad.status, bad.exit_code), (JobStatus::Failed, Some(2)));
        let after = catalogue.require_job(after.id).unwrap();
        assert_eq!(after.status, JobStatus::Failed);
        assert_eq!(
            after.error.as_deref(),
            Some(&*format!("after job {} failed", bad.id))
        );
        queue.shutdown().unwrap();
    }

    #[test]
    fn the_same_job_again_is_a_cache_hit_unless_forced() {
        let fake = Fake::new();
        let (queue, _, db, subject) = start("cache", fake.clone());
        queue
            .submit(request(subject, "render", Class::Heavy, json!({"x": 1})))
            .unwrap();
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        let again = queue
            .submit(request(subject, "render", Class::Heavy, json!({"x": 1})))
            .unwrap();
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        assert_eq!(fake.runs.load(Ordering::SeqCst), 1);
        let catalogue = Catalogue::open(&db).unwrap();
        assert!(
            catalogue
                .checkpoint_of_job(again.id)
                .unwrap()
                .unwrap()
                .cached
        );
        let mut forced = request(subject, "render", Class::Heavy, json!({"x": 1}));
        forced.force = true;
        queue.submit(forced).unwrap();
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        assert_eq!(fake.runs.load(Ordering::SeqCst), 2);
        queue.shutdown().unwrap();
    }

    #[test]
    fn cancel_stops_a_queued_and_a_running_job() {
        let fake = Fake::new();
        let (queue, _, db, subject) = start("cancel", fake.clone());
        let long = queue
            .submit(request(
                subject,
                "render",
                Class::Heavy,
                json!({"x": 1, "delay_ms": 10_000}),
            ))
            .unwrap();
        let waiting = queue
            .submit(request(subject, "render", Class::Heavy, json!({"x": 2})))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Catalogue::open(&db)
            .unwrap()
            .require_job(long.id)
            .unwrap()
            .status
            != JobStatus::Running
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(queue.cancel(waiting.id).unwrap(), JobStatus::Cancelled);
        queue.cancel(long.id).unwrap();
        assert!(queue.wait_idle(Duration::from_secs(5)).unwrap());
        let catalogue = Catalogue::open(&db).unwrap();
        assert_eq!(
            catalogue.require_job(long.id).unwrap().status,
            JobStatus::Cancelled
        );
        assert_eq!(
            catalogue.require_job(waiting.id).unwrap().status,
            JobStatus::Cancelled
        );
        assert!(catalogue.checkpoint_of_job(long.id).unwrap().is_none());
        // A retry runs the same request again.
        let retried = queue.retry(waiting.id).unwrap();
        assert!(queue.wait_idle(Duration::from_secs(5)).unwrap());
        assert_eq!(
            catalogue.require_job(retried.id).unwrap().status,
            JobStatus::Succeeded
        );
        queue.shutdown().unwrap();
    }

    #[test]
    fn a_restart_interrupts_what_was_running_and_runs_what_was_queued() {
        let fake = Fake::new();
        let (store, db) = workspace("restart");
        let catalogue = Catalogue::open(&db).unwrap();
        let subject = catalogue.create_subject("project", "p").unwrap().id;
        let running = catalogue
            .insert_job(
                &request(subject, "render", Class::Heavy, json!({"x": 1})),
                "",
                JobStatus::Queued,
            )
            .unwrap();
        catalogue
            .move_job(running.id, &[JobStatus::Queued], JobStatus::Running)
            .unwrap();
        let queued = catalogue
            .insert_job(
                &request(subject, "render", Class::Heavy, json!({"x": 2})),
                "",
                JobStatus::Queued,
            )
            .unwrap();
        let queue = Queue::start(store, &db, Limits::default(), fake, "test").unwrap();
        assert!(queue.wait_idle(Duration::from_secs(10)).unwrap());
        assert_eq!(
            catalogue.require_job(running.id).unwrap().status,
            JobStatus::Interrupted
        );
        assert_eq!(
            catalogue.require_job(queued.id).unwrap().status,
            JobStatus::Succeeded
        );
        queue.shutdown().unwrap();
    }

    #[test]
    fn bad_submissions_are_refused() {
        let fake = Fake::new();
        let (queue, store, _, subject) = start("bad", fake);
        assert!(queue
            .submit(request(999, "render", Class::Heavy, json!({})))
            .is_err());
        let mut missing = request(subject, "render", Class::Heavy, json!({}));
        missing
            .inputs
            .insert("in".to_string(), InputRef::Hash("e".repeat(64)));
        assert!(queue.submit(missing).is_err());
        let mut no_job = request(subject, "render", Class::Heavy, json!({}));
        no_job.after = vec![999];
        assert!(queue.submit(no_job).is_err());
        assert!(queue
            .set_limits(Limits {
                heavy: 0,
                light: 1,
                network: 1
            })
            .is_err());
        let stored = store.put_bytes(b"x", "file").unwrap();
        let _ = ObjectInfo { ..stored };
        queue.shutdown().unwrap();
    }
}
