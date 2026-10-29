//! `reelctl serve` end to end: JSON-RPC over the workspace socket, jobs run as
//! child `reelctl` processes, checkpoints, the cache, events, cancel, restart, the
//! paid-call gate and the peer check. Synthetic fixtures only; no model is called.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("REELCTL_RVM_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
        .env_remove("MANIMGX_BIN")
        .env_remove("SMELTER_BIN")
        .env_remove("OPENROUTER_API_KEY")
        .stdin(Stdio::null());
    command
}

/// A short folder: the socket path must stay under about 100 bytes.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn generate(kind: &str, path: &Path) {
    let output = Command::new("bash")
        .arg(repo_root().join("fixtures/generate.sh"))
        .arg(kind)
        .arg(path)
        .output()
        .expect("spawn fixture generator");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Serve {
    child: Child,
    workspace: PathBuf,
}

impl Serve {
    fn start(workspace: &Path, extra: &[&str]) -> Self {
        let mut child = reelctl()
            .arg("serve")
            .arg("--workspace")
            .arg(workspace)
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn serve");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .expect("serve's first line");
        assert!(line.starts_with("serve: workspace="), "{line}");
        Self {
            child,
            workspace: workspace.to_path_buf(),
        }
    }

    fn socket(&self) -> PathBuf {
        self.workspace.join(".reelstudio/reelctl.sock")
    }

    fn client(&self) -> Client {
        Client::connect(&self.socket())
    }

    /// `shutdown`, then the exit code.
    fn stop(mut self) -> Option<i32> {
        let _ = self.client().call("shutdown", json!({}));
        self.wait()
    }

    fn wait(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            assert!(Instant::now() < deadline, "serve did not stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Children of a killed serve, by the workspace path in their argv.
        let _ = Command::new("pkill")
            .arg("-f")
            .arg(self.workspace.join(".reelstudio").as_os_str())
            .status();
    }
}

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: i64,
    notifications: VecDeque<Value>,
}

impl Client {
    fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            next: 0,
            notifications: VecDeque::new(),
        }
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("read a line");
        assert!(read > 0, "the service closed the connection");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line}"))
    }

    /// The result, or the error object.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let request =
            json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        writeln!(self.writer, "{request}").unwrap();
        loop {
            let message = self.read();
            if message.get("id") == Some(&json!(self.next)) {
                return match message.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(message["result"].clone()),
                };
            }
            self.notifications.push_back(message);
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .unwrap_or_else(|error| panic!("{method}: {error}"))
    }

    /// The next notification, waiting for it.
    fn notification(&mut self) -> Value {
        self.notifications
            .pop_front()
            .unwrap_or_else(|| self.read())
    }

    fn wait_job(&mut self, job: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let found = self.ok("jobs.get", json!({"job": job}));
            if ["succeeded", "failed", "cancelled", "interrupted"]
                .contains(&found["status"].as_str().unwrap_or(""))
            {
                return found;
            }
            assert!(Instant::now() < deadline, "job {job} did not finish");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn give_words(dna: &Path, out: &Path) {
    let mut value: Value = serde_json::from_slice(&fs::read(dna).unwrap()).unwrap();
    let mut words = Vec::new();
    for n in 0..10u64 {
        let start = 500 + 1_100 * n;
        words.push(json!({"text": format!("word{n}"), "start_ms": start, "end_ms": start + 400}));
    }
    value["words"] = json!(words);
    value["asr_backend"] = json!("whisper_cpp");
    fs::write(out, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn subject(client: &mut Client, kind: &str) -> i64 {
    client.ok("subjects.create", json!({"kind": kind, "name": kind}))["id"]
        .as_i64()
        .unwrap()
}

#[test]
fn serve_runs_an_edit_chain_over_its_socket_with_checkpoints_events_and_the_cache() {
    let root = scratch("chain");
    let video = root.join("draft.mp4");
    generate("draft12", &video);
    let catalog = root.join("catalog.json");
    fs::write(
        &catalog,
        json!({"models": {"qwen/qwen3.5-9b": [
            {"provider": "DeepInfra", "quantization": "bf16", "context_length": 262144,
             "prompt_usd_per_token": 0.0000001, "completion_usd_per_token": 0.00000015}
        ]}})
        .to_string(),
    )
    .unwrap();
    let workspace = root.join("ws");
    let serve = Serve::start(
        &workspace,
        &["--understand-catalog", catalog.to_str().unwrap()],
    );
    assert_eq!(mode(&workspace.join(".reelstudio")), 0o700);
    assert_eq!(mode(&serve.socket()), 0o600);

    let second = reelctl()
        .arg("serve")
        .arg("--workspace")
        .arg(&workspace)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&second.stderr).contains("another reelctl serve"));

    let mut client = serve.client();
    let info = client.ok("workspace.info", json!({}));
    assert_eq!(info["schema_version"], 1);
    assert_eq!(
        info["limits"],
        json!({"heavy": 1, "light": 4, "network": 1})
    );
    let doctor = client.ok("doctor.check", json!({}));
    assert_eq!(doctor["ffmpeg"]["found"], true);
    assert_eq!(doctor["whisper_model"]["found"], false);

    let mut events = serve.client();
    events.ok("events.subscribe", json!({}));

    let asset = client.ok("assets.import", json!({"path": video, "kind": "video"}))["id"]
        .as_i64()
        .unwrap();
    let reel = subject(&mut client, "reel");
    let project = subject(&mut client, "project");
    let decompile = client.ok(
        "jobs.submit",
        json!({"subject": reel, "command": "decompile", "inputs": {"input": {"asset": asset}}}),
    )["job"]
        .as_i64()
        .unwrap();
    let init = client.ok(
        "jobs.submit",
        json!({"subject": project, "command": "timeline.init", "inputs": {"input": {"asset": asset}}}),
    )["job"]
        .as_i64()
        .unwrap();
    let cut = client.ok(
        "jobs.submit",
        json!({"subject": project, "command": "timeline.cut-silence",
               "inputs": {"project": {"from": init, "role": "out"}, "dna": {"from": decompile, "role": "out"}}}),
    );
    assert_eq!(cut["status"], "blocked");
    let cut = cut["job"].as_i64().unwrap();
    let done = client.wait_job(cut);
    assert_eq!(done["status"], "succeeded", "{done}");
    let decompiled = client.wait_job(decompile);
    assert!(
        decompiled["result"]["cuts"].as_str().is_some(),
        "{decompiled}"
    );
    assert!(decompiled["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["event"]["event"] == "progress" && e["event"]["stage"] == "decode"));

    // The subscriber was told of the decode's progress and the cut's checkpoint.
    let mut seen_progress = false;
    let mut seen_checkpoint = false;
    while !(seen_progress && seen_checkpoint) {
        let note = events.notification();
        seen_progress |= note["method"] == "job.progress" && note["params"]["job"] == decompile;
        seen_checkpoint |= note["method"] == "checkpoint.created" && note["params"]["job"] == cut;
    }

    // Outputs are stored read-only; the scratch folders are gone.
    let checkpoint = done["checkpoint"]["id"].as_i64().unwrap();
    let files = client.ok("checkpoints.get", json!({"checkpoint": checkpoint}))["files"].clone();
    let otio = PathBuf::from(files["out"]["path"].as_str().unwrap());
    assert_eq!(mode(&otio), 0o400);
    assert!(fs::read_to_string(&otio).unwrap().contains("OTIO_SCHEMA"));
    let views = client.ok(
        "checkpoints.get",
        json!({"checkpoint": decompiled["checkpoint"]["id"]}),
    )["files"]["views"]["kind"]
        .clone();
    assert_eq!(views, "tree");
    assert_eq!(
        fs::read_dir(workspace.join(".reelstudio/jobs"))
            .unwrap()
            .count(),
        0
    );

    // The same request again is a cache hit: a checkpoint and no child.
    let again = client.ok(
        "jobs.submit",
        json!({"subject": project, "command": "timeline.init", "inputs": {"input": {"asset": asset}}}),
    );
    assert_eq!(again["cached"], true);
    let again = client.wait_job(again["job"].as_i64().unwrap());
    assert_eq!(again["status"], "succeeded");
    assert_eq!(again["checkpoint"]["cached"], true);
    assert!(again["events"].as_array().unwrap().is_empty());

    // Revert appends; nothing is deleted.
    let first = client.ok("checkpoints.list", json!({"subject": project}))[0]["id"].clone();
    client.ok(
        "checkpoints.revert",
        json!({"subject": project, "checkpoint": first}),
    );
    assert_eq!(
        client
            .ok("checkpoints.list", json!({"subject": project}))
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // The paid call: refused without a plan, then planned (no key, no model) and
    // refused above the bound.
    let decompiled_dna = PathBuf::from(
        client.ok(
            "checkpoints.get",
            json!({"checkpoint": decompiled["checkpoint"]["id"]}),
        )["files"]["out"]["path"]
            .as_str()
            .unwrap(),
    );
    let worded = root.join("worded.edit_dna.json");
    give_words(&decompiled_dna, &worded);
    let dna = client.ok("assets.import", json!({"path": worded, "kind": "edit_dna"}))["id"]
        .as_i64()
        .unwrap();
    let understanding = subject(&mut client, "understanding");
    let refused = client
        .call(
            "jobs.submit",
            json!({"subject": understanding, "command": "understand", "args": {"max_cost_usd": 1.0},
                   "inputs": {"input": {"asset": asset}, "dna": {"asset": dna}}}),
        )
        .unwrap_err();
    assert!(refused["message"]
        .as_str()
        .unwrap()
        .contains("understand.plan"));
    let key = "sk-or-serve-fixture-never-stored";
    let no_plan = client
        .call(
            "understand.run",
            json!({"plan": 999, "approved_max_usd": 0.01, "api_key": key}),
        )
        .unwrap_err();
    assert!(no_plan["message"].as_str().unwrap().contains("no plan"));
    let plan = client.ok(
        "understand.plan",
        json!({"subject": understanding, "input": {"asset": asset}, "dna": {"asset": dna}, "every_s": 4}),
    );
    let bound = plan["cost_upper_bound_usd"].as_f64().unwrap();
    assert!(bound > 0.0, "{plan}");
    let over = client
        .call(
            "understand.run",
            json!({"plan": plan["plan"], "approved_max_usd": bound + 1.0, "api_key": key}),
        )
        .unwrap_err();
    assert_eq!(over["data"]["exit_code"], 4);

    assert_eq!(serve.stop(), Some(0));
    assert!(!workspace.join(".reelstudio/reelctl.sock").exists());
    let db = fs::read(workspace.join(".reelstudio/studio.db")).unwrap();
    assert!(!db.windows(key.len()).any(|w| w == key.as_bytes()));
}

/// Wait until `job` reports render progress past frame 0.
fn wait_for_render(events: &mut Client, job: i64) {
    loop {
        let note = events.notification();
        if note["method"] == "job.progress"
            && note["params"]["job"] == job
            && note["params"]["event"]["stage"] == "render"
            && note["params"]["event"]["done"].as_u64().unwrap_or(0) > 0
        {
            return;
        }
    }
}

fn start_render(root: &Path, serve: &Serve) -> (Client, i64) {
    let video = root.join("bench.mp4");
    generate("bench60", &video);
    let mut client = serve.client();
    let asset = client.ok("assets.import", json!({"path": video, "kind": "video"}))["id"]
        .as_i64()
        .unwrap();
    let project = subject(&mut client, "project");
    let init = client.ok(
        "jobs.submit",
        json!({"subject": project, "command": "timeline.init", "inputs": {"input": {"asset": asset}}}),
    )["job"]
        .as_i64()
        .unwrap();
    assert_eq!(client.wait_job(init)["status"], "succeeded");
    let render = client.ok(
        "jobs.submit",
        json!({"subject": project, "command": "render", "inputs": {"project": {"from": init, "role": "out"}}}),
    )["job"]
        .as_i64()
        .unwrap();
    let mut events = serve.client();
    events.ok("events.subscribe", json!({"job": render}));
    wait_for_render(&mut events, render);
    (client, render)
}

#[test]
fn cancelling_a_render_stops_its_child_and_leaves_no_scratch() {
    let root = scratch("cancel");
    let workspace = root.join("ws");
    let serve = Serve::start(&workspace, &[]);
    let (mut client, render) = start_render(&root, &serve);
    client.ok("jobs.cancel", json!({"job": render}));
    let done = client.wait_job(render);
    assert_eq!(done["status"], "cancelled", "{done}");
    assert_eq!(done["exit_code"], 130, "{done}");
    assert!(done["checkpoint"].is_null());
    assert_eq!(
        fs::read_dir(workspace.join(".reelstudio/jobs"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(serve.stop(), Some(0));
}

#[test]
fn a_killed_serve_leaves_its_running_job_interrupted_after_a_restart() {
    let root = scratch("restart");
    let workspace = root.join("ws");
    let mut serve = Serve::start(&workspace, &[]);
    let (_client, render) = start_render(&root, &serve);
    let _ = serve.child.kill();
    let _ = serve.wait();
    drop(serve);
    let serve = Serve::start(&workspace, &[]);
    let mut client = serve.client();
    let job = client.ok("jobs.get", json!({"job": render}));
    assert_eq!(job["status"], "interrupted", "{job}");
    let retried = client.ok("jobs.retry", json!({"job": render}));
    assert_ne!(retried["job"], render);
    client.ok("jobs.cancel", json!({"job": retried["job"]}));
    assert_eq!(serve.stop(), Some(0));
}

#[test]
fn a_client_of_another_uid_is_refused() {
    let root_user = Command::new("id")
        .arg("-u")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
        .unwrap_or(false);
    let tools = ["setpriv", "python3"].iter().all(|tool| {
        Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .is_ok_and(|output| output.status.success())
    });
    if !root_user || !tools {
        eprintln!("skipped: needs root, setpriv and python3 to connect as another uid");
        return;
    }
    let root = scratch("peer");
    let workspace = root.join("ws");
    let serve = Serve::start(&workspace, &[]);
    // Open the folder and the socket so the other user can reach them at all; the
    // peer check is then all that stands in the way.
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        workspace.join(".reelstudio"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::set_permissions(serve.socket(), fs::Permissions::from_mode(0o777)).unwrap();
    let script = "import socket,sys\ns=socket.socket(socket.AF_UNIX)\ns.connect(sys.argv[1])\ns.sendall(b'{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"workspace.info\"}\\n')\ns.settimeout(10)\ntry:\n    print(repr(s.recv(4096)))\nexcept ConnectionResetError:\n    print(repr(b''))";
    let output = Command::new("setpriv")
        .args([
            "--reuid=65534",
            "--regid=65534",
            "--clear-groups",
            "python3",
            "-c",
            script,
        ])
        .arg(serve.socket())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(said.trim(), "b''", "another uid got an answer");
    // The owner still gets one.
    assert_eq!(
        serve.client().ok("workspace.info", json!({}))["schema_version"],
        1
    );
    assert_eq!(serve.stop(), Some(0));
}
