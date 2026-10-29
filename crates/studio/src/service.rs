//! The service's request handlers: one method name and its params in, a JSON
//! result or a JSON-RPC error out. The socket, the peer check, `doctor.check`,
//! `events.subscribe` and `shutdown` are the server's (`reelctl serve`); every
//! other method is here, so it is tested without a socket or a process.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::catalogue::{now_ms, Catalogue};
use crate::checkpoint::cache_key;
use crate::commands::{self, CommandSpec};
use crate::error::{Result, StudioError};
use crate::jobs::{Class, InputRef, Job, JobRequest, JobStatus};
use crate::pipeline::{self, ReelOptions, Source};
use crate::protocol::{code, RpcError};
use crate::queue::{Limits, Queue, Runner};
use crate::store::Store;

/// How long an `understand.plan` stays usable.
pub const PLAN_LIFETIME_MS: i64 = 15 * 60 * 1000;
/// How long `understand.plan` waits for its dry run.
const PLAN_WAIT: Duration = Duration::from_secs(120);
/// The cap a plan's dry run is given when the request names none (a dry run
/// spends nothing; the cap only decides whether it says a real run would be
/// refused).
const PLAN_DEFAULT_CAP_USD: f64 = 0.50;
/// The most events `jobs.get` returns, the latest.
const LAST_EVENTS: usize = 50;
/// The name of the key's environment variable.
pub const KEY_NAME: &str = "OPENROUTER_API_KEY";

/// API keys waiting for the one `understand` child each belongs to, by job id.
/// Held in memory only. The service holds the lock while it submits the job and
/// puts the key, and the runner takes the key through the same lock, so a job
/// that starts at once still finds it.
#[derive(Default)]
pub struct Secrets {
    keys: Mutex<HashMap<i64, String>>,
}

impl Secrets {
    fn lock(&self) -> MutexGuard<'_, HashMap<i64, String>> {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The key for `job`, once: it is forgotten as it is taken.
    pub fn take(&self, job: i64) -> Option<String> {
        self.lock().remove(&job)
    }
}

/// A plan from `understand.plan`, used at most once by `understand.run`.
#[derive(Debug, Clone)]
struct Plan {
    subject: i64,
    inputs: BTreeMap<String, InputRef>,
    /// The understand arguments, without `dry_run` and `max_cost_usd`.
    args: serde_json::Map<String, Value>,
    /// The bound shown to the client, rounded up to the next 0.0001 USD.
    bound_usd: f64,
    expires_at: i64,
}

/// How the service is set up.
pub struct Config {
    pub workspace: PathBuf,
    /// The `reelctl` version, part of every cache key.
    pub version: String,
    pub limits: Limits,
    /// The names of the environment variables the runner can pass on (set in
    /// the service's own environment), recorded on each job that may get them.
    pub env_available: Vec<String>,
}

pub struct Service {
    workspace: PathBuf,
    store: Store,
    db_path: PathBuf,
    queue: Queue,
    version: String,
    secrets: Arc<Secrets>,
    plans: Mutex<HashMap<i64, Plan>>,
    env_available: Vec<String>,
}

fn int(params: &Value, key: &str) -> std::result::Result<i64, RpcError> {
    params
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| RpcError::params(format!("\"{key}\" must be a whole number")))
}

fn opt_int(params: &Value, key: &str) -> std::result::Result<Option<i64>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => int(params, key).map(Some),
    }
}

fn text<'a>(params: &'a Value, key: &str) -> std::result::Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::params(format!("\"{key}\" must be a string")))
}

fn opt_text<'a>(params: &'a Value, key: &str) -> std::result::Result<Option<&'a str>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => text(params, key).map(Some),
    }
}

fn flag(params: &Value, key: &str, default: bool) -> std::result::Result<bool, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(RpcError::params(format!("\"{key}\" must be true or false"))),
    }
}

fn number(params: &Value, key: &str) -> std::result::Result<f64, RpcError> {
    params
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| RpcError::params(format!("\"{key}\" must be a number")))
}

fn inputs_of(value: Option<&Value>) -> std::result::Result<BTreeMap<String, InputRef>, RpcError> {
    match value {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
            RpcError::params(
                "\"inputs\" maps each role to a hash, {\"from\": job, \"role\": name} or {\"asset\": id}",
            )
        }),
    }
}

fn to_value<T: serde::Serialize + ?Sized>(value: &T) -> std::result::Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| RpcError::new(code::INTERNAL, error.to_string()))
}

/// Check `args` and the input roles against the command catalogue, with
/// placeholder paths: the same checks `build` makes when the job runs.
fn validate(
    spec: &CommandSpec,
    args: &Value,
    inputs: &BTreeMap<String, InputRef>,
) -> std::result::Result<(), RpcError> {
    let placeholders: BTreeMap<String, PathBuf> = inputs
        .keys()
        .map(|role| (role.clone(), PathBuf::from(format!("/inputs/{role}"))))
        .collect();
    let outputs = commands::default_outputs(spec, Path::new("/scratch"));
    commands::build(spec.name, args, &placeholders, &outputs)
        .map(|_| ())
        .map_err(|error| RpcError::params(error.to_string()))
}

impl Service {
    /// Open the workspace (the store and the catalogue, migrated) and start the
    /// queue with `runner`. Jobs left running by a previous service become
    /// interrupted.
    pub fn open(config: Config, runner: Arc<dyn Runner>, secrets: Arc<Secrets>) -> Result<Self> {
        if !config.workspace.is_absolute() {
            return Err(StudioError::invalid(format!(
                "the workspace {} is not an absolute path",
                config.workspace.display()
            )));
        }
        let store = Store::open(&config.workspace)?;
        let db_path = store.root().join("studio.db");
        Catalogue::open(&db_path)?;
        let queue = Queue::start(
            store.clone(),
            &db_path,
            config.limits,
            runner,
            &config.version,
        )?;
        Ok(Self {
            workspace: config.workspace,
            store,
            db_path,
            queue,
            version: config.version,
            secrets,
            plans: Mutex::new(HashMap::new()),
            env_available: config.env_available,
        })
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn catalogue(&self) -> Result<Catalogue> {
        Catalogue::open(&self.db_path)
    }

    /// Answer one request. An unknown method is `METHOD_NOT_FOUND`.
    pub fn handle(&self, method: &str, params: &Value) -> std::result::Result<Value, RpcError> {
        match method {
            "workspace.info" => self.info(),
            "workspace.set_limits" => self.set_limits(params),
            "assets.import" => self.import(params),
            "assets.list" => to_value(&self.catalogue()?.assets(opt_text(params, "kind")?)?),
            "subjects.create" => to_value(
                &self
                    .catalogue()?
                    .create_subject(text(params, "kind")?, text(params, "name")?)?,
            ),
            "subjects.list" => to_value(&self.catalogue()?.subjects(opt_text(params, "kind")?)?),
            "subjects.branch" => {
                let (subject, checkpoint) = self
                    .catalogue()?
                    .branch(int(params, "checkpoint")?, text(params, "name")?)?;
                Ok(json!({ "subject": to_value(&subject)?, "checkpoint": to_value(&checkpoint)? }))
            }
            "jobs.submit" => self.submit(params),
            "jobs.get" => self.job(params),
            "jobs.list" => {
                let status = opt_text(params, "status")?
                    .map(JobStatus::parse)
                    .transpose()?;
                let limit = opt_int(params, "limit")?.unwrap_or(100).clamp(1, 1000);
                to_value(
                    &self
                        .catalogue()?
                        .jobs(opt_int(params, "subject")?, status, limit)?,
                )
            }
            "jobs.cancel" => {
                let job = int(params, "job")?;
                // A live call cancelled before it started never takes its key.
                drop(self.secrets.take(job));
                let status = self.queue.cancel(job)?;
                Ok(json!({ "status": status }))
            }
            "jobs.retry" => self.retry(params),
            "jobs.argv" => self.argv(params),
            "checkpoints.list" => {
                to_value(&self.catalogue()?.checkpoints(int(params, "subject")?)?)
            }
            "checkpoints.get" => self.checkpoint(params),
            "checkpoints.revert" => to_value(
                &self
                    .catalogue()?
                    .revert(int(params, "subject")?, int(params, "checkpoint")?)?,
            ),
            "checkpoints.diff" => to_value(
                &self
                    .catalogue()?
                    .diff_checkpoints(int(params, "a")?, int(params, "b")?)?,
            ),
            "objects.path" => {
                let hash = text(params, "hash")?;
                if !self.store.has(hash) {
                    return Err(RpcError::refused(format!("{hash} is not in the store")));
                }
                Ok(json!({ "path": self.store.path(hash)? }))
            }
            "store.stats" => self.stats(),
            "store.gc" => self.gc(params),
            "understand.plan" => self.plan(params),
            "understand.run" => self.run_plan(params),
            "pipelines.make_reels" => self.make_reels(params),
            _ => Err(RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("there is no method \"{method}\""),
            )),
        }
    }

    fn info(&self) -> std::result::Result<Value, RpcError> {
        Ok(json!({
            "path": self.workspace,
            "version": self.version,
            "schema_version": self.catalogue()?.version()?,
            "limits": to_value(&self.queue.limits()?)?,
            "pid": std::process::id(),
        }))
    }

    fn set_limits(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let mut limits = self.queue.limits()?;
        for (key, slot) in [
            ("heavy", &mut limits.heavy),
            ("light", &mut limits.light),
            ("network", &mut limits.network),
        ] {
            if let Some(value) = opt_int(params, key)? {
                *slot = usize::try_from(value)
                    .map_err(|_| RpcError::params(format!("\"{key}\" must be at least 1")))?;
            }
        }
        self.queue.set_limits(limits)?;
        to_value(&limits)
    }

    fn import(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let path = PathBuf::from(text(params, "path")?);
        let kind = text(params, "kind")?;
        let copy = flag(params, "copy", false)?;
        let name = match opt_text(params, "name")? {
            Some(name) => name.to_string(),
            None => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let catalogue = self.catalogue()?;
        // The catalogue checks that the path is absolute and a regular file first.
        let asset = catalogue.import_asset(kind, &name, &path, false)?;
        if copy {
            let info = self.store.put_file(&path)?;
            if info.hash != asset.hash {
                return Err(RpcError::refused(format!(
                    "{} changed while it was imported",
                    path.display()
                )));
            }
            catalogue.record_object(&info)?;
            let asset = catalogue.import_asset(kind, &name, &path, true)?;
            return to_value(&asset);
        }
        to_value(&asset)
    }

    /// Whether a job would be a cache hit now: when it is not forced and every
    /// input is already known.
    fn would_be_cached(&self, catalogue: &Catalogue, job: &Job) -> Result<bool> {
        if job.force {
            return Ok(false);
        }
        let mut hashes = BTreeMap::new();
        for (role, input) in &job.inputs {
            let hash = match input {
                InputRef::Hash(hash) => hash.clone(),
                InputRef::Asset { asset } => match catalogue.asset(*asset)? {
                    Some(asset) => asset.hash,
                    None => return Ok(false),
                },
                InputRef::FromJob { .. } => return Ok(false),
            };
            hashes.insert(role.clone(), hash);
        }
        let key = cache_key(&job.command, &job.args, &hashes, &self.version);
        Ok(catalogue.cached_outputs(&key)?.is_some())
    }

    fn env_names(&self, spec: &CommandSpec) -> Vec<String> {
        spec.env
            .iter()
            .filter(|name| {
                **name != KEY_NAME && self.env_available.iter().any(|have| have == *name)
            })
            .map(|name| name.to_string())
            .collect()
    }

    fn submit(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let subject = int(params, "subject")?;
        let command = text(params, "command")?;
        if command == "understand" {
            return Err(RpcError::refused(
                "understand runs through understand.plan, then understand.run with an approved amount",
            ));
        }
        let spec = commands::command(command)?;
        let args = match params.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(args) => args.clone(),
        };
        // The Smelter backend starts a server that listens on every interface;
        // the service opens no network port, so its jobs render with ffmpeg.
        if command == "render"
            && (args.get("backend").and_then(Value::as_str) == Some("smelter")
                || args.get("smelter").is_some_and(|value| !value.is_null()))
        {
            return Err(RpcError::refused(
                "the service renders with ffmpeg only: the Smelter backend listens on the network (run `reelctl render --backend smelter` yourself)",
            ));
        }
        let inputs = inputs_of(params.get("inputs"))?;
        validate(spec, &args, &inputs)?;
        let after: Vec<i64> = match params.get("after") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| RpcError::params("\"after\" must be a list of job ids"))?,
        };
        let job = self.queue.submit(JobRequest {
            subject,
            command: command.to_string(),
            class: commands::class_of(spec, &args),
            args,
            inputs,
            priority: opt_int(params, "priority")?.unwrap_or(0),
            after,
            env_names: self.env_names(spec),
            force: flag(params, "force", false)?,
        })?;
        let cached = self.would_be_cached(&self.catalogue()?, &job)?;
        Ok(json!({ "job": job.id, "status": job.status, "cached": cached }))
    }

    fn job(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let catalogue = self.catalogue()?;
        let job = catalogue.require_job(int(params, "job")?)?;
        let events = catalogue.events(job.id, 0)?;
        let skip = events.len().saturating_sub(LAST_EVENTS);
        let checkpoint = catalogue.checkpoint_of_job(job.id)?;
        let mut value = to_value(&job)?;
        value["events"] = to_value(&events[skip..])?;
        value["checkpoint"] = to_value(&checkpoint)?;
        Ok(value)
    }

    /// `jobs.argv {job}`: the job as `reelctl` arguments, for "Copy as CLI
    /// command": inputs as the files they are (a stored object, an imported
    /// file, an earlier job's output), outputs as their names in the current
    /// folder. It holds no key: keys are never arguments.
    fn argv(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let catalogue = self.catalogue()?;
        let job = catalogue.require_job(int(params, "job")?)?;
        let mut inputs = BTreeMap::new();
        for (role, input) in &job.inputs {
            let path = match input {
                InputRef::Hash(hash) => self.store.path(hash)?,
                InputRef::Asset { asset } => match catalogue.asset(*asset)? {
                    Some(asset) => PathBuf::from(asset.source_path),
                    None => PathBuf::from(format!("<asset {asset}>")),
                },
                InputRef::FromJob { from, role: output } => {
                    match catalogue
                        .checkpoint_of_job(*from)?
                        .and_then(|checkpoint| checkpoint.outputs.get(output).cloned())
                    {
                        Some(hash) => self.store.path(&hash)?,
                        None => PathBuf::from(format!("<job {from} {output}>")),
                    }
                }
            };
            inputs.insert(role.clone(), path);
        }
        let spec = commands::command(&job.command)?;
        let outputs = commands::outputs_for(spec, &job.args, Path::new("."));
        let built = commands::build(&job.command, &job.args, &inputs, &outputs)?;
        Ok(json!({ "argv": built.argv }))
    }

    fn retry(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let id = int(params, "job")?;
        let old = self.catalogue()?.require_job(id)?;
        if old.command == "understand" {
            return Err(RpcError::refused(
                "a model call is not retried; ask for a new understand.plan",
            ));
        }
        let job = self.queue.retry(id)?;
        Ok(json!({ "job": job.id, "status": job.status }))
    }

    fn checkpoint(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let catalogue = self.catalogue()?;
        let id = int(params, "checkpoint")?;
        let checkpoint = catalogue
            .checkpoint(id)?
            .ok_or_else(|| RpcError::refused(format!("there is no checkpoint {id}")))?;
        let mut files = serde_json::Map::new();
        for (role, hash) in &checkpoint.outputs {
            let kind = catalogue.object(hash)?.map(|object| object.kind);
            files.insert(
                role.clone(),
                json!({ "hash": hash, "kind": kind, "path": self.store.path(hash)? }),
            );
        }
        let mut value = to_value(&checkpoint)?;
        value["files"] = Value::Object(files);
        Ok(value)
    }

    /// `hashes` and every object the trees among them hold.
    fn with_tree_entries(
        &self,
        catalogue: &Catalogue,
        hashes: BTreeSet<String>,
    ) -> Result<BTreeSet<String>> {
        let mut all = BTreeSet::new();
        let mut pending: Vec<String> = hashes.into_iter().collect();
        while let Some(hash) = pending.pop() {
            if !all.insert(hash.clone()) {
                continue;
            }
            let is_tree = catalogue
                .object(&hash)?
                .is_some_and(|object| object.kind == "tree");
            if is_tree && self.store.has(&hash) {
                pending.extend(self.store.tree(&hash)?.into_iter().map(|entry| entry.hash));
            }
        }
        Ok(all)
    }

    fn stats(&self) -> std::result::Result<Value, RpcError> {
        let catalogue = self.catalogue()?;
        let objects = self.store.objects()?;
        let sizes: HashMap<&str, u64> = objects
            .iter()
            .map(|(hash, size)| (hash.as_str(), *size))
            .collect();
        let mut by_kind: BTreeMap<String, u64> = BTreeMap::new();
        for (hash, size) in &objects {
            let kind = catalogue
                .object(hash)?
                .map_or_else(|| "unknown".to_string(), |object| object.kind);
            *by_kind.entry(kind).or_insert(0) += size;
        }
        let mut by_subject = Vec::new();
        for subject in catalogue.subjects(None)? {
            let hashes: BTreeSet<String> = catalogue
                .checkpoints(subject.id)?
                .into_iter()
                .flat_map(|checkpoint| checkpoint.outputs.into_values())
                .collect();
            let bytes: u64 = self
                .with_tree_entries(&catalogue, hashes)?
                .iter()
                .filter_map(|hash| sizes.get(hash.as_str()))
                .sum();
            by_subject.push(json!({ "subject": subject.id, "name": subject.name, "kind": subject.kind, "bytes": bytes }));
        }
        Ok(json!({
            "objects": objects.len(),
            "bytes": objects.iter().map(|(_, size)| size).sum::<u64>(),
            "by_kind": by_kind,
            "by_subject": by_subject,
        }))
    }

    fn gc(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let dry_run = flag(params, "dry_run", true)?;
        let catalogue = self.catalogue()?;
        if !dry_run {
            let busy = [JobStatus::Queued, JobStatus::Blocked, JobStatus::Running]
                .iter()
                .map(|status| {
                    catalogue
                        .jobs(None, Some(*status), 1)
                        .map(|jobs| jobs.len())
                })
                .sum::<Result<usize>>()?;
            if busy > 0 {
                return Err(RpcError::refused(
                    "jobs are queued or running; collect garbage when the queue is idle",
                ));
            }
        }
        let keep = self.with_tree_entries(&catalogue, catalogue.referenced_hashes()?)?;
        let unused: Vec<(String, u64)> = self
            .store
            .objects()?
            .into_iter()
            .filter(|(hash, _)| !keep.contains(hash))
            .collect();
        if !dry_run {
            for (hash, _) in &unused {
                self.store.remove(hash)?;
                catalogue.forget_object(hash)?;
            }
        }
        Ok(json!({
            "dry_run": dry_run,
            "objects": unused.len(),
            "bytes": unused.iter().map(|(_, size)| size).sum::<u64>(),
            "hashes": unused.iter().map(|(hash, _)| hash).collect::<Vec<_>>(),
        }))
    }

    /// Wait for `job` to finish, for at most `wait`.
    fn wait_for(&self, job: i64, wait: Duration) -> Result<Job> {
        let deadline = Instant::now() + wait;
        loop {
            let found = self.catalogue()?.require_job(job)?;
            if found.status.finished() || Instant::now() >= deadline {
                return Ok(found);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn plan(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let subject = int(params, "subject")?;
        let mut inputs = BTreeMap::new();
        for role in ["input", "dna"] {
            let value = params
                .get(role)
                .ok_or_else(|| RpcError::params(format!("\"{role}\" is needed")))?;
            let input: InputRef = serde_json::from_value(value.clone()).map_err(|_| {
                RpcError::params(format!(
                    "\"{role}\" is a hash, {{\"from\": job, \"role\": name}} or {{\"asset\": id}}"
                ))
            })?;
            inputs.insert(role.to_string(), input);
        }
        let mut args = serde_json::Map::new();
        for key in [
            "model",
            "quant",
            "every_s",
            "sheet",
            "effort",
            "max_output_tokens",
            "decoder",
        ] {
            if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
                args.insert(key.to_string(), value.clone());
            }
        }
        let cap = match params.get("max_cost_usd") {
            None | Some(Value::Null) => PLAN_DEFAULT_CAP_USD,
            Some(_) => number(params, "max_cost_usd")?,
        };
        let mut dry = args.clone();
        dry.insert("dry_run".to_string(), json!(true));
        dry.insert("max_cost_usd".to_string(), json!(cap));
        let dry = Value::Object(dry);
        let spec = commands::command("understand")?;
        validate(spec, &dry, &inputs)?;
        let job = self.queue.submit(JobRequest {
            subject,
            command: "understand".to_string(),
            class: commands::class_of(spec, &dry),
            args: dry,
            inputs: inputs.clone(),
            priority: 0,
            after: Vec::new(),
            env_names: Vec::new(),
            // A plan is never a cache hit: the result line is what it is for.
            force: true,
        })?;
        let done = self.wait_for(job.id, PLAN_WAIT)?;
        if done.status != JobStatus::Succeeded {
            let message = done
                .error
                .unwrap_or_else(|| format!("the plan's dry run is {}", done.status.as_str()));
            let error = RpcError::refused(message);
            return Err(match done.exit_code {
                Some(exit_code @ 2..=4) => error.with_exit_code(exit_code),
                _ => error,
            });
        }
        let fields = done.result.unwrap_or(Value::Null);
        let field = |key: &str| -> std::result::Result<f64, RpcError> {
            fields
                .get(key)
                .and_then(Value::as_str)
                .and_then(|text| text.parse::<f64>().ok())
                .ok_or_else(|| RpcError::new(code::INTERNAL, format!("the dry run gave no {key}")))
        };
        // The line rounds to 0.0001; round up past it so the bound shown is never
        // under the one `understand` computes.
        let printed = field("cost_upper_bound_usd")?;
        if !printed.is_finite() || printed < 0.0 {
            return Err(RpcError::new(
                code::INTERNAL,
                format!("the dry run gave an unusable bound: {printed}"),
            ));
        }
        let bound_usd = ((printed * 10_000.0).round() + 1.0) / 10_000.0;
        let expires_at = now_ms() + PLAN_LIFETIME_MS;
        self.plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                job.id,
                Plan {
                    subject,
                    inputs,
                    args,
                    bound_usd,
                    expires_at,
                },
            );
        Ok(json!({
            "plan": job.id,
            "cost_upper_bound_usd": bound_usd,
            "frames": field("frames")? as i64,
            "images": field("images")? as i64,
            "expires_at": expires_at,
        }))
    }

    /// The file an input names, for the service to read itself (the
    /// understanding of `pipelines.make_reels`). An earlier job's output must
    /// already exist.
    fn input_path(
        &self,
        catalogue: &Catalogue,
        input: &InputRef,
    ) -> std::result::Result<PathBuf, RpcError> {
        match input {
            InputRef::Hash(hash) => Ok(self.store.path(hash)?),
            InputRef::Asset { asset } => catalogue
                .asset(*asset)?
                .map(|asset| PathBuf::from(asset.source_path))
                .ok_or_else(|| RpcError::refused(format!("there is no asset {asset}"))),
            InputRef::FromJob { from, role } => {
                let job = catalogue.require_job(*from)?;
                if job.status != JobStatus::Succeeded {
                    return Err(RpcError::refused(format!(
                        "job {from} is {}; wait until it has succeeded",
                        job.status.as_str()
                    )));
                }
                let hash = catalogue
                    .checkpoint_of_job(*from)?
                    .and_then(|checkpoint| checkpoint.outputs.get(role).cloned())
                    .ok_or_else(|| {
                        RpcError::refused(format!("job {from} has no output \"{role}\""))
                    })?;
                Ok(self.store.path(&hash)?)
            }
        }
    }

    /// `pipelines.make_reels`: the make-reels chain in three calls. Without
    /// `dna` it queues the decompile; with a `dna` and no `understanding` it
    /// makes the understand plan (the client then approves and runs it); with
    /// both it queues every chosen clip's steps on its own `reel` subject,
    /// chained by their outputs, so every step is a checkpoint.
    fn make_reels(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let input_of = |key: &str| -> std::result::Result<Option<InputRef>, RpcError> {
            match params.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(value) => serde_json::from_value(value.clone()).map(Some).map_err(|_| {
                    RpcError::params(format!(
                        "\"{key}\" is a hash, {{\"from\": job, \"role\": name}} or {{\"asset\": id}}"
                    ))
                }),
            }
        };
        let input = input_of("input")?.ok_or_else(|| RpcError::params("\"input\" is needed"))?;
        let outliers: Vec<InputRef> = match params.get("outliers") {
            Some(Value::Array(list)) => list
                .iter()
                .map(|value| serde_json::from_value(value.clone()))
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| RpcError::params("\"outliers\" is a list of inputs"))?,
            _ => Vec::new(),
        };
        if outliers.is_empty() || outliers.len() > 3 {
            return Err(RpcError::params("one to three \"outliers\" are needed"));
        }
        let target_url = text(params, "target_url")?.to_string();
        let catalogue = self.catalogue()?;
        let subject = match opt_int(params, "subject")? {
            Some(subject) => subject,
            None => {
                let name = opt_text(params, "name")?.unwrap_or("make reels");
                catalogue.create_subject("make_reels", name)?.id
            }
        };

        let Some(dna) = input_of("dna")? else {
            let mut args = serde_json::Map::new();
            if let Some(model) = opt_text(params, "whisper_model")? {
                args.insert("whisper_model".to_string(), json!(model));
            }
            let args = Value::Object(args);
            let spec = commands::command("decompile")?;
            let inputs = BTreeMap::from([("input".to_string(), input.clone())]);
            validate(spec, &args, &inputs)?;
            let job = self.queue.submit(JobRequest {
                subject,
                command: "decompile".to_string(),
                class: commands::class_of(spec, &args),
                args,
                inputs,
                priority: 0,
                after: Vec::new(),
                env_names: self.env_names(spec),
                force: false,
            })?;
            return Ok(json!({ "stage": "decompile", "subject": subject, "jobs": [job.id] }));
        };

        let Some(understanding) = input_of("understanding")? else {
            if let InputRef::FromJob { .. } = &dna {
                // The plan's dry run reads the DNA now.
                self.input_path(&catalogue, &dna)?;
            }
            let mut plan_params = serde_json::Map::new();
            plan_params.insert("subject".to_string(), json!(subject));
            plan_params.insert("input".to_string(), to_value(&input)?);
            plan_params.insert("dna".to_string(), to_value(&dna)?);
            for key in [
                "model",
                "every_s",
                "max_cost_usd",
                "quant",
                "sheet",
                "effort",
                "max_output_tokens",
                "decoder",
            ] {
                if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
                    plan_params.insert(key.to_string(), value.clone());
                }
            }
            let plan = self.plan(&Value::Object(plan_params))?;
            return Ok(json!({ "stage": "plan", "subject": subject, "plan": plan }));
        };

        let path = self.input_path(&catalogue, &understanding)?;
        let bytes = std::fs::read(&path).map_err(|error| StudioError::io(&path, error))?;
        let parsed: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RpcError::refused(format!("the understanding is not JSON: {error}"))
        })?;
        let clips: Vec<usize> = match params.get("clips") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| RpcError::params("\"clips\" is a list of clip numbers"))?,
        };
        let reels = opt_int(params, "reels")?.map(|n| n.max(0) as usize);
        let allow_flagged = flag(params, "allow_flagged", false)?;
        let chosen = pipeline::choose(&parsed, &clips, reels, allow_flagged)?;
        if chosen.is_empty() {
            return Err(RpcError::refused(
                "no clip to make (none passes the clip rules, or reels is 0); see allow_flagged and clips",
            ));
        }

        let mut notes = Vec::new();
        let weights = opt_text(params, "weights")?.map(str::to_string);
        if weights
            .as_deref()
            .is_some_and(|path| !Path::new(path).is_absolute())
        {
            return Err(RpcError::params("\"weights\" must be an absolute path"));
        }
        let mut follow_presenter = match opt_text(params, "follow")?.unwrap_or("presenter") {
            "presenter" => true,
            "centre" => false,
            other => {
                return Err(RpcError::params(format!(
                    "\"follow\" is presenter or centre, not \"{other}\""
                )))
            }
        };
        if follow_presenter {
            let weights_found = weights
                .as_deref()
                .is_some_and(|path| Path::new(path).is_file());
            let runtime_found = self
                .env_available
                .iter()
                .any(|name| name == "ORT_DYLIB_PATH");
            if !(weights_found && runtime_found) {
                follow_presenter = false;
                notes.push(format!(
                    "reframe followed the centre, not the presenter: {}",
                    match (weights_found, runtime_found) {
                        (false, false) => "no RobustVideoMatting weights and no ORT_DYLIB_PATH",
                        (false, true) => "no RobustVideoMatting weights (weights)",
                        _ => "no ONNX Runtime library (ORT_DYLIB_PATH)",
                    }
                ));
            }
        }
        let style = match params.get("style") {
            None | Some(Value::Null) => json!({}),
            Some(style @ Value::Object(_)) => style.clone(),
            Some(_) => {
                return Err(RpcError::params(
                    "\"style\" is an object of overlay captions arguments",
                ))
            }
        };
        let options = ReelOptions {
            allow_flagged,
            outliers: outliers.len(),
            follow_presenter,
            weights: if follow_presenter { weights } else { None },
            target_url,
            card_ms: opt_int(params, "card_ms")?.map_or(2500, |ms| ms.max(0) as u64),
            style,
            size: opt_text(params, "size")?.unwrap_or("1080x1920").to_string(),
        };

        // Every step of every reel is checked before the first is queued, so a
        // refused request runs nothing (as `make-reels` builds every argv first).
        for &clip in &chosen {
            for step in pipeline::reel_steps(clip, &options) {
                let placeholders: BTreeMap<String, InputRef> = step
                    .inputs
                    .iter()
                    .map(|(role, _)| (role.clone(), InputRef::Hash(String::new())))
                    .collect();
                validate(commands::command(step.command)?, &step.args, &placeholders).map_err(
                    |error| RpcError::params(format!("{}: {}", step.name, error.message)),
                )?;
            }
        }
        let mut made = Vec::new();
        for clip in chosen {
            let reel = catalogue.create_subject("reel", &format!("clip {clip}"))?;
            let steps = pipeline::reel_steps(clip, &options);
            let mut jobs: Vec<i64> = Vec::new();
            let mut listed = Vec::new();
            for step in &steps {
                let mut inputs = BTreeMap::new();
                for (role, source) in &step.inputs {
                    let input = match source {
                        Source::Input => input.clone(),
                        Source::Dna => dna.clone(),
                        Source::Understanding => understanding.clone(),
                        Source::Outlier(n) => outliers[n - 1].clone(),
                        Source::Step(index) => InputRef::FromJob {
                            from: jobs[*index],
                            role: steps[*index].output.to_string(),
                        },
                    };
                    inputs.insert(role.clone(), input);
                }
                let spec = commands::command(step.command)?;
                validate(spec, &step.args, &inputs)?;
                let job = self.queue.submit(JobRequest {
                    subject: reel.id,
                    command: step.command.to_string(),
                    class: commands::class_of(spec, &step.args),
                    args: step.args.clone(),
                    inputs,
                    priority: 0,
                    after: Vec::new(),
                    env_names: self.env_names(spec),
                    force: false,
                })?;
                jobs.push(job.id);
                listed.push(json!({ "step": step.name, "job": job.id }));
            }
            made.push(json!({ "clip": clip, "subject": reel.id, "jobs": listed }));
        }
        Ok(json!({
            "stage": "reels",
            "subject": subject,
            "follow": if follow_presenter { "presenter" } else { "centre" },
            "notes": notes,
            "reels": made,
        }))
    }

    fn run_plan(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let id = int(params, "plan")?;
        let approved = number(params, "approved_max_usd")?;
        let key = match params.get("api_key").and_then(Value::as_str) {
            Some(key) if !key.trim().is_empty() => key.to_string(),
            _ => return Err(RpcError::refused("no API key").with_exit_code(3)),
        };
        let plan = {
            let mut plans = self
                .plans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let plan = plans.get(&id).cloned().ok_or_else(|| {
                RpcError::refused(format!("there is no plan {id}, or it was used"))
            })?;
            if now_ms() > plan.expires_at {
                plans.remove(&id);
                return Err(RpcError::refused(format!(
                    "plan {id} expired; ask for a new one"
                )));
            }
            if approved <= 0.0 || approved > plan.bound_usd + 1e-9 {
                return Err(RpcError::refused(format!(
                    "the approved amount ${approved:.4} must be above 0 and no more than the plan's bound ${:.4}",
                    plan.bound_usd
                ))
                .with_exit_code(4));
            }
            if approved + 1e-9 < plan.bound_usd {
                return Err(RpcError::refused(format!(
                    "the approved amount ${approved:.4} is under the plan's bound ${:.4}; the call would be refused",
                    plan.bound_usd
                ))
                .with_exit_code(4));
            }
            plans.remove(&id);
            plan
        };
        let mut args = plan.args;
        args.insert("max_cost_usd".to_string(), json!(approved));
        let args = Value::Object(args);
        // The lock is held from the submit to the key's insertion; the runner
        // takes the key through it.
        let mut keys = self.secrets.lock();
        let job = self.queue.submit(JobRequest {
            subject: plan.subject,
            command: "understand".to_string(),
            class: Class::Network,
            args,
            inputs: plan.inputs,
            priority: 0,
            after: Vec::new(),
            env_names: vec![KEY_NAME.to_string()],
            force: true,
        })?;
        keys.insert(job.id, key);
        drop(keys);
        Ok(json!({ "job": job.id, "status": job.status }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{RunContext, RunOutcome};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Writes each output, answers a dry run with a bound, and records whether
    /// an understand child would have got its key.
    struct Fake {
        secrets: Arc<Secrets>,
        keys_seen: Mutex<Vec<Option<String>>>,
        runs: AtomicUsize,
    }

    impl Runner for Fake {
        fn run(&self, job: &Job, context: RunContext<'_>) -> RunOutcome {
            self.runs.fetch_add(1, Ordering::SeqCst);
            let spec = commands::command(&job.command).unwrap();
            if job.command == "understand" && job.args["dry_run"] == json!(true) {
                return RunOutcome {
                    exit_code: Some(0),
                    result: Some(
                        json!({"frames": "12", "images": "12", "cost_upper_bound_usd": "0.0123"}),
                    ),
                    outputs: BTreeMap::new(),
                    error: None,
                };
            }
            if job.command == "understand" {
                self.keys_seen
                    .lock()
                    .unwrap()
                    .push(self.secrets.take(job.id));
            }
            let mut outputs = BTreeMap::new();
            for (role, path) in commands::default_outputs(spec, context.scratch) {
                let body = format!("{} {} {}", job.command, job.args, context.inputs.len());
                std::fs::write(&path, body).unwrap();
                outputs.insert(role, path);
            }
            RunOutcome {
                exit_code: Some(0),
                result: Some(json!({})),
                outputs,
                error: None,
            }
        }
    }

    fn service(name: &str) -> (Service, Arc<Fake>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("studio-service-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let secrets = Arc::new(Secrets::default());
        let fake = Arc::new(Fake {
            secrets: Arc::clone(&secrets),
            keys_seen: Mutex::new(Vec::new()),
            runs: AtomicUsize::new(0),
        });
        let service = Service::open(
            Config {
                workspace: dir.clone(),
                version: "test".to_string(),
                limits: Limits::default(),
                env_available: Vec::new(),
            },
            Arc::clone(&fake) as Arc<dyn Runner>,
            secrets,
        )
        .unwrap();
        (service, fake, dir)
    }

    fn call(service: &Service, method: &str, params: Value) -> Value {
        service
            .handle(method, &params)
            .unwrap_or_else(|error| panic!("{method}: {}", error.message))
    }

    fn import(service: &Service, dir: &Path, name: &str, kind: &str) -> i64 {
        let path = dir.join(name);
        std::fs::write(&path, format!("bytes of {name}")).unwrap();
        call(
            service,
            "assets.import",
            json!({"path": path, "kind": kind}),
        )["id"]
            .as_i64()
            .unwrap()
    }

    fn finish(service: &Service, job: i64) -> Job {
        let done = service.wait_for(job, Duration::from_secs(10)).unwrap();
        assert!(done.status.finished(), "{done:?}");
        done
    }

    #[test]
    fn a_live_call_needs_an_unexpired_plan_and_the_approved_bound_and_its_key_is_never_stored() {
        let (service, fake, dir) = service("plan");
        let video = import(&service, &dir, "long.mp4", "video");
        let dna = import(&service, &dir, "long.edit_dna.json", "edit_dna");
        let subject = call(
            &service,
            "subjects.create",
            json!({"kind": "understanding", "name": "u"}),
        )["id"]
            .as_i64()
            .unwrap();
        let key = "sk-or-test-key-that-must-never-be-stored";

        let refused = service
            .handle(
                "understand.run",
                &json!({"plan": 99, "approved_max_usd": 1.0, "api_key": key}),
            )
            .unwrap_err();
        assert!(
            refused.message.contains("no plan 99"),
            "{}",
            refused.message
        );
        assert!(service
            .handle("jobs.submit", &json!({"subject": subject, "command": "understand", "args": {"max_cost_usd": 1.0, "dry_run": true}, "inputs": {"input": {"asset": video}, "dna": {"asset": dna}}}))
            .unwrap_err()
            .message
            .contains("understand.plan"));

        let plan = call(
            &service,
            "understand.plan",
            json!({"subject": subject, "input": {"asset": video}, "dna": {"asset": dna}, "every_s": 5, "max_output_tokens": 24000}),
        );
        // 0.0123 printed: shown rounded up past it.
        assert_eq!(plan["cost_upper_bound_usd"], json!(0.0124));
        assert_eq!(plan["frames"], 12);
        let id = plan["plan"].as_i64().unwrap();

        let over = service
            .handle(
                "understand.run",
                &json!({"plan": id, "approved_max_usd": 0.5, "api_key": key}),
            )
            .unwrap_err();
        assert_eq!(over.data, Some(json!({"exit_code": 4})));
        let under = service
            .handle(
                "understand.run",
                &json!({"plan": id, "approved_max_usd": 0.001, "api_key": key}),
            )
            .unwrap_err();
        assert_eq!(under.data, Some(json!({"exit_code": 4})));
        let no_key = service
            .handle(
                "understand.run",
                &json!({"plan": id, "approved_max_usd": 0.0124}),
            )
            .unwrap_err();
        assert_eq!(no_key.data, Some(json!({"exit_code": 3})));

        let run = call(
            &service,
            "understand.run",
            json!({"plan": id, "approved_max_usd": 0.0124, "api_key": key}),
        );
        let job = finish(&service, run["job"].as_i64().unwrap());
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.args["max_cost_usd"], json!(0.0124));
        // The output limit chosen at plan time is the one the run uses.
        assert_eq!(job.args["max_output_tokens"], json!(24000));
        assert_eq!(job.args.get("dry_run"), None);
        assert_eq!(job.env_names, vec![KEY_NAME.to_string()]);
        assert_eq!(*fake.keys_seen.lock().unwrap(), vec![Some(key.to_string())]);

        let again = service
            .handle(
                "understand.run",
                &json!({"plan": id, "approved_max_usd": 0.0124, "api_key": key}),
            )
            .unwrap_err();
        assert!(again.message.contains("was used"), "{}", again.message);
        assert!(service
            .handle("jobs.retry", &json!({"job": job.id}))
            .is_err());

        // The key is in no file of the workspace.
        let mut stack = vec![dir.join(crate::store::STUDIO_DIR)];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            } else {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes.windows(key.len()).any(|w| w == key.as_bytes()),
                    "the key is in {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn jobs_take_assets_by_reference_refuse_bad_requests_and_say_when_they_will_be_cached() {
        let (service, fake, dir) = service("submit");
        let video = import(&service, &dir, "draft.mp4", "video");
        let subject = call(
            &service,
            "subjects.create",
            json!({"kind": "project", "name": "p"}),
        )["id"]
            .as_i64()
            .unwrap();
        let submit = |params: Value| service.handle("jobs.submit", &params);
        let bad = submit(json!({"subject": subject, "command": "timeline.init", "args": {"nope": 1}, "inputs": {"input": {"asset": video}}})).unwrap_err();
        assert_eq!(bad.code, code::INVALID_PARAMS);
        assert!(bad.message.contains("nope"), "{}", bad.message);
        let unknown =
            submit(json!({"subject": subject, "command": "rm", "inputs": {}})).unwrap_err();
        assert_eq!(unknown.code, code::REFUSED);

        let first = submit(json!({"subject": subject, "command": "timeline.init", "inputs": {"input": {"asset": video}}})).unwrap();
        assert_eq!(first["cached"], false);
        let first = finish(&service, first["job"].as_i64().unwrap());
        assert_eq!(first.status, JobStatus::Succeeded);
        let second = submit(json!({"subject": subject, "command": "timeline.init", "inputs": {"input": {"asset": video}}})).unwrap();
        assert_eq!(second["cached"], true);
        finish(&service, second["job"].as_i64().unwrap());
        assert_eq!(fake.runs.load(Ordering::SeqCst), 1);

        // A changed asset fails the job rather than being read as the old one.
        std::fs::write(dir.join("draft.mp4"), "other bytes").unwrap();
        let changed = submit(json!({"subject": subject, "command": "timeline.init", "inputs": {"input": {"asset": video}}, "force": true})).unwrap();
        let changed = finish(&service, changed["job"].as_i64().unwrap());
        assert_eq!(changed.status, JobStatus::Failed);
        assert!(changed.error.unwrap().contains("changed"));

        let missing = service.handle("nope.method", &json!({})).unwrap_err();
        assert_eq!(missing.code, code::METHOD_NOT_FOUND);
    }

    #[test]
    fn revert_and_branch_add_checkpoints_and_gc_keeps_what_they_refer_to() {
        let (service, _fake, dir) = service("gc");
        let video = import(&service, &dir, "a.mp4", "video");
        let subject = call(
            &service,
            "subjects.create",
            json!({"kind": "project", "name": "p"}),
        )["id"]
            .as_i64()
            .unwrap();
        let init = call(
            &service,
            "jobs.submit",
            json!({"subject": subject, "command": "timeline.init", "inputs": {"input": {"asset": video}}}),
        );
        let init = finish(&service, init["job"].as_i64().unwrap());
        let first = service
            .catalogue()
            .unwrap()
            .checkpoint_of_job(init.id)
            .unwrap()
            .unwrap();
        let project = first.outputs["out"].clone();
        let cut = call(
            &service,
            "jobs.submit",
            json!({"subject": subject, "command": "timeline.cut-silence", "inputs": {"project": project, "dna": project}}),
        );
        finish(&service, cut["job"].as_i64().unwrap());

        let reverted = call(
            &service,
            "checkpoints.revert",
            json!({"subject": subject, "checkpoint": first.id}),
        );
        assert_eq!(reverted["outputs"], to_value(&first.outputs).unwrap());
        let list = call(&service, "checkpoints.list", json!({"subject": subject}));
        assert_eq!(list.as_array().unwrap().len(), 3);
        let got = call(&service, "checkpoints.get", json!({"checkpoint": first.id}));
        assert!(Path::new(got["files"]["out"]["path"].as_str().unwrap()).is_file());
        let branch = call(
            &service,
            "subjects.branch",
            json!({"checkpoint": first.id, "name": "try"}),
        );
        assert_eq!(branch["subject"]["parent_subject"], subject);

        // An object nothing refers to is listed by a dry run and removed only
        // when asked.
        let loose = service.store().put_bytes(b"loose", "file").unwrap();
        let dry = call(&service, "store.gc", json!({}));
        assert_eq!(dry["dry_run"], true);
        assert_eq!(dry["hashes"], json!([loose.hash]));
        assert!(service.store().has(&loose.hash));
        let done = call(&service, "store.gc", json!({"dry_run": false}));
        assert_eq!(done["objects"], 1);
        assert!(!service.store().has(&loose.hash));
        assert!(service.store().has(&project));
        let stats = call(&service, "store.stats", json!({}));
        assert!(stats["bytes"].as_u64().unwrap() > 0);
        let info = call(&service, "workspace.info", json!({}));
        assert_eq!(info["schema_version"], 1);
        let limits = call(&service, "workspace.set_limits", json!({"light": 2}));
        assert_eq!(limits["light"], 2);
        assert!(service
            .handle("workspace.set_limits", &json!({"heavy": 0}))
            .is_err());
    }
}
