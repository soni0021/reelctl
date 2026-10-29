//! End to end: `reelctl overlay add-chart|remove` and `render` with a FAKE `manimgx`
//! (a shell script this test writes: it logs its arguments and draws the chart video
//! with `ffmpeg` on the key colour). The real manimgx is never run. Needs `ffmpeg`
//! and `ffprobe`, as `fixtures/generate.sh` does.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

/// `reelctl` without the model and weight paths a shell profile may export, and
/// without a worker path.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("REELCTL_RVM_WEIGHTS")
        .env_remove("MANIMGX_BIN");
    command
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reelctl-overlay-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    fs::canonicalize(&dir).expect("canonical")
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn run_command(mut command: Command) -> Output {
    command.output().expect("spawn")
}

fn run(args: &[&str]) -> Output {
    let mut command = reelctl();
    command.args(args);
    run_command(command)
}

fn ok(args: &[&str]) -> String {
    let output = run(args);
    assert!(
        output.status.success(),
        "reelctl {args:?} failed ({:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8")
}

fn fail(args: &[&str], code: i32) -> String {
    let output = run(args);
    assert_eq!(
        output.status.code(),
        Some(code),
        "reelctl {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stderr).expect("utf8")
}

/// A 10 s, 640x360 dark-green video with a tone, at `rate` (`30` or `30000/1001`).
fn make_video(dir: &Path, rate: &str) -> PathBuf {
    let path = dir.join("take.mp4");
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("color=c=0x204020:s=640x360:r={rate}:d=10"))
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=10"])
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&path)
        .output()
        .expect("ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    path
}

const FAKE: &str = r#"#!/bin/sh
D='__DIR__'
{ for a in "$@"; do printf '%s\t' "$a"; done; printf '\n'; } >> "$D/argv.log"
printf '%s\n' "${OPENROUTER_API_KEY-unset}" >> "$D/key.log"
case "$1" in
  --version) echo "fake-manimgx 0.0.1"; exit 0 ;;
  inspect)
    out=""
    while [ $# -gt 0 ]; do if [ "$1" = -o ]; then out="$2"; fi; shift; done
    code=0
    if [ -f "$D/inspect.exit" ]; then code=$(cat "$D/inspect.exit"); fi
    if [ "$code" != 0 ]; then echo "text overlaps the title at 1.0 s"; printf png > "$out"; fi
    exit "$code" ;;
  render)
    cp "$2" "$D/scene.copy.py"
    out=""; size=""; fps=""
    while [ $# -gt 0 ]; do
      case "$1" in -o) out="$2" ;; -r) size="$2" ;; --fps) fps="$2" ;; esac
      shift
    done
    if [ -f "$D/render.exit" ]; then echo "no GPU adapter" >&2; exit "$(cat "$D/render.exit")"; fi
    bg=0xff00ff
    if [ -f "$D/render.bg" ]; then bg=$(cat "$D/render.bg"); fi
    dur=$(cat "$D/render.duration")
    ffmpeg -v error -y -f lavfi -i "color=c=$bg:s=$size:r=$fps:d=$dur" \
      -vf "drawbox=x=iw/4:y=ih/4:w=iw/2:h=ih/2:color=white:t=fill" \
      -c:v libx264 -pix_fmt yuv420p "$out"
    exit $? ;;
esac
exit 64
"#;

struct Env {
    dir: PathBuf,
    project: PathBuf,
    fake: PathBuf,
}

impl Env {
    /// A project over a fresh video at `rate`, a fake worker whose videos last
    /// `duration` seconds.
    fn new(name: &str, rate: &str, duration: &str) -> Self {
        let dir = scratch(name);
        let video = make_video(&dir, rate);
        let project = dir.join("edit.otio");
        ok(&[
            "timeline",
            "init",
            "--input",
            &s(&video),
            "--out",
            &s(&project),
        ]);
        let worker = dir.join("worker");
        fs::create_dir_all(&worker).expect("worker dir");
        let fake = worker.join("manimgx");
        fs::write(&fake, FAKE.replace("__DIR__", &s(&worker))).expect("script");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::write(worker.join("render.duration"), duration).expect("duration");
        Self { dir, project, fake }
    }

    fn worker_dir(&self) -> PathBuf {
        self.dir.join("worker")
    }

    fn set(&self, file: &str, content: &str) {
        fs::write(self.worker_dir().join(file), content).expect("set");
    }

    /// The worker's argument lines so far, tab-separated fields.
    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.worker_dir().join("argv.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                line.trim_end_matches('\t')
                    .split('\t')
                    .map(str::to_string)
                    .collect()
            })
            .collect()
    }

    fn count(&self, command: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.first().map(String::as_str) == Some(command))
            .count()
    }

    fn spec_file(&self, name: &str, spec: &Value) -> PathBuf {
        let path = self.dir.join(name);
        fs::write(&path, spec.to_string()).expect("spec");
        path
    }

    fn add(&self, spec: &Path, out: &Path, extra: &[&str]) -> Output {
        let mut command = reelctl();
        command
            .args(["overlay", "add-chart", "--project"])
            .arg(&self.project)
            .arg("--spec")
            .arg(spec)
            .arg("--out")
            .arg(out)
            .arg("--manimgx")
            .arg(&self.fake)
            .args(extra);
        run_command(command)
    }

    fn add_ok(&self, spec: &Path, extra: &[&str]) -> String {
        let output = self.add(spec, &self.project, extra);
        assert!(
            output.status.success(),
            "add-chart failed ({:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8")
    }

    fn render(&self, out: &Path) {
        ok(&["render", "--project", &s(&self.project), "--out", &s(out)]);
    }
}

fn chart(title: &str, start_ms: u64, end_ms: u64) -> Value {
    json!({
        "kind": "bar",
        "title": title,
        "series": [{"label": "Mon", "value": 3}, {"label": "Tue", "value": 5.5}],
        "unit": "%",
        "start_ms": start_ms,
        "end_ms": end_ms,
        "width_pct": 50,
    })
}

/// The pixels of column `x` for every frame of `video`: `[frame][row] = rgb`.
fn column(video: &Path, x: u32) -> Vec<Vec<[u8; 3]>> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(video)
        .args([
            "-vf",
            &format!("format=rgb24,crop=1:360:{x}:0"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output()
        .expect("ffmpeg");
    assert!(output.status.success());
    output
        .stdout
        .chunks(360 * 3)
        .map(|frame| frame.chunks(3).map(|p| [p[0], p[1], p[2]]).collect())
        .collect()
}

fn is_white(pixel: [u8; 3]) -> bool {
    pixel.iter().all(|c| *c > 200)
}

fn is_green(pixel: [u8; 3]) -> bool {
    pixel[1] > pixel[0] + 10 && pixel[1] > pixel[2] + 10 && pixel[1] < 120
}

fn duration_frames(video: &Path) -> usize {
    column(video, 0).len()
}

#[test]
fn a_chart_is_added_once_and_rendered_over_exactly_its_frames() {
    let env = Env::new("basic", "30", "2");
    let spec = env.spec_file("chart.json", &chart("Reels per week", 2000, 4000));
    let line = env.add_ok(&spec, &[]);
    assert!(
        line.contains("unchanged=false") && line.contains("frames=60..120"),
        "{line}"
    );
    assert!(line.contains("renderer=\"fake-manimgx 0.0.1\""), "{line}");

    // The worker was asked for the documented flags and nothing else.
    let calls = env.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[0], ["--version"]);
    assert_eq!(calls[1][0], "inspect");
    assert_eq!(calls[1][2], "ReelctlChart");
    assert_eq!(&calls[1][3..5], ["-t", "end"]);
    assert_eq!(calls[1][5], "-o");
    assert_eq!(calls[2][0], "render");
    assert_eq!(calls[2][2], "ReelctlChart");
    assert_eq!(&calls[2][3..4], ["-o"]);
    // 50 % of 640 is 320 wide, 240 high; the project rate 30.
    assert_eq!(&calls[2][5..9], ["-r", "320x240", "--fps", "30"]);
    assert_eq!(calls[2].len(), 9);
    for call in &calls[1..] {
        assert!(
            call[1].starts_with('/') && call[1].ends_with("scene.py"),
            "{call:?}"
        );
    }

    // The project has the overlay track; the asset is kept beside it, alone.
    let project: Value =
        serde_json::from_str(&fs::read_to_string(&env.project).expect("p")).expect("json");
    let tracks = project["tracks"]["children"].as_array().expect("tracks");
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[2]["metadata"]["reelctl"]["overlays"], true);
    let assets: Vec<String> = fs::read_dir(env.dir.join("edit.overlays"))
        .expect("assets")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(assets.len(), 1, "{assets:?}");
    assert!(
        assets[0].ends_with(".mp4") && !assets[0].starts_with('.'),
        "{assets:?}"
    );

    // Render: the picture is the video except on frames 60 to 119.
    let out = env.dir.join("out.mp4");
    env.render(&out);
    let frames = column(&out, 320);
    assert_eq!(frames.len(), 300);
    for (frame, white) in [
        (0, false),
        (59, false),
        (60, true),
        (90, true),
        (119, true),
        (120, false),
        (299, false),
    ] {
        let pixel = frames[frame][180];
        assert_eq!(is_white(pixel), white, "frame {frame}: {pixel:?}");
        if !white {
            assert!(
                is_green(pixel),
                "frame {frame} should be the video: {pixel:?}"
            );
        }
    }
    // The keyed-out part of the chart shows the video, not the key colour.
    let corner = frames[90][100];
    assert!(is_green(corner), "{corner:?}");
}

#[test]
fn adding_the_same_chart_again_changes_nothing_and_does_not_call_the_worker() {
    let env = Env::new("idempotent", "30", "2");
    let spec = env.spec_file("chart.json", &chart("Same", 2000, 4000));
    env.add_ok(&spec, &[]);
    let once = fs::read(&env.project).expect("project");
    let calls_once = env.calls().len();
    let line = env.add_ok(&spec, &[]);
    assert!(line.contains("unchanged=true"), "{line}");
    assert_eq!(fs::read(&env.project).expect("project"), once);
    assert_eq!(env.calls().len(), calls_once, "the worker was called again");
    // To another file: the same bytes.
    let other = env.dir.join("copy.otio");
    let output = env.add(&spec, &other, &[]);
    assert!(output.status.success());
    assert_eq!(fs::read(&other).expect("copy"), once);
    let project: Value = serde_json::from_slice(&once).expect("json");
    let track = &project["tracks"]["children"][2]["children"];
    assert_eq!(
        track.as_array().expect("items").len(),
        2,
        "a gap and one clip"
    );
}

#[test]
fn a_lost_asset_is_drawn_again() {
    let env = Env::new("lost", "30", "2");
    let spec = env.spec_file("chart.json", &chart("Lost", 2000, 4000));
    env.add_ok(&spec, &[]);
    let once = fs::read(&env.project).expect("project");
    let assets = env.dir.join("edit.overlays");
    for entry in fs::read_dir(&assets).expect("assets") {
        fs::remove_file(entry.expect("entry").path()).expect("rm");
    }
    let line = env.add_ok(&spec, &[]);
    assert!(line.contains("unchanged=false"), "{line}");
    assert_eq!(env.count("render"), 2);
    assert_eq!(fs::read(&env.project).expect("project"), once);
    assert_eq!(fs::read_dir(&assets).expect("assets").count(), 1);
}

#[test]
fn an_ntsc_project_gets_the_same_exact_frames() {
    let env = Env::new("ntsc", "30000/1001", "2.002");
    let spec = env.spec_file("chart.json", &chart("NTSC", 2000, 4000));
    let line = env.add_ok(&spec, &[]);
    assert!(line.contains("frames=60..120"), "{line}");
    let calls = env.calls();
    // The tool takes a whole rate: 29.97 is drawn at 30 and retimed by the render.
    assert_eq!(&calls[2][7..9], ["--fps", "30"]);
    let out = env.dir.join("out.mp4");
    env.render(&out);
    let frames = column(&out, 320);
    assert_eq!(frames.len(), 299);
    for (frame, white) in [
        (59, false),
        (60, true),
        (61, true),
        (90, true),
        (119, true),
        (120, false),
        (121, false),
    ] {
        assert_eq!(
            is_white(frames[frame][180]),
            white,
            "frame {frame}: {:?}",
            frames[frame][180]
        );
    }
}

#[test]
fn the_position_decides_where_the_chart_sits() {
    let env = Env::new("positions", "30", "2");
    // Rows of the white box (of 360) per position: top 89..209, centre 120..240,
    // bottom 151..271.
    let probes = [100usize, 130, 160, 230, 260];
    let expected = [
        ("top", [true, true, true, false, false]),
        ("center", [false, true, true, true, false]),
        ("bottom", [false, false, true, true, true]),
    ];
    for (position, whites) in expected {
        let mut spec = chart(&format!("At {position}"), 2000, 4000);
        spec["position"] = json!(position);
        let spec = env.spec_file(&format!("{position}.json"), &spec);
        let out_project = env.dir.join(format!("{position}.otio"));
        let output = env.add(&spec, &out_project, &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rendered = env.dir.join(format!("{position}.mp4"));
        ok(&[
            "render",
            "--project",
            &s(&out_project),
            "--out",
            &s(&rendered),
        ]);
        let frames = column(&rendered, 320);
        for (row, want) in probes.iter().zip(whites) {
            assert_eq!(
                is_white(frames[90][*row]),
                want,
                "{position} row {row}: {:?}",
                frames[90][*row]
            );
        }
    }
}

#[test]
fn removing_the_chart_gives_the_project_and_the_picture_back() {
    let env = Env::new("remove", "30", "2");
    let original = fs::read(&env.project).expect("project");
    let spec = env.spec_file("chart.json", &chart("Gone", 2000, 4000));
    let line = env.add_ok(&spec, &[]);
    let id = line
        .split_whitespace()
        .find_map(|word| word.strip_prefix("id="))
        .expect("id")
        .to_string();
    assert_eq!(id.len(), 16);
    fail(
        &[
            "overlay",
            "remove",
            "--project",
            &s(&env.project),
            "--id",
            "0000000000000000",
            "--out",
            &s(&env.project),
        ],
        2,
    );
    let removed = ok(&[
        "overlay",
        "remove",
        "--project",
        &s(&env.project),
        "--id",
        &id,
        "--out",
        &s(&env.project),
    ]);
    assert!(removed.contains("removed=true"), "{removed}");
    assert_eq!(fs::read(&env.project).expect("project"), original);
    let out = env.dir.join("out.mp4");
    env.render(&out);
    assert!(is_green(column(&out, 320)[90][180]));
}

#[test]
fn two_charts_may_not_overlap_and_touching_ones_may() {
    let env = Env::new("overlap", "30", "2");
    let first = env.spec_file("a.json", &chart("A", 2000, 4000));
    env.add_ok(&first, &[]);
    let clash = env.spec_file("b.json", &chart("B", 3000, 5000));
    let calls = env.calls().len();
    let output = env.add(&clash, &env.project, &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps"));
    assert_eq!(
        env.calls().len(),
        calls,
        "nothing was drawn for a span that cannot be used"
    );
    let touching = env.spec_file("c.json", &chart("C", 4000, 5000));
    env.set("render.duration", "1");
    env.add_ok(&touching, &[]);
    let out = env.dir.join("out.mp4");
    env.render(&out);
    let frames = column(&out, 320);
    for frame in [60usize, 119, 120, 149] {
        assert!(is_white(frames[frame][180]), "frame {frame}");
    }
    assert!(!is_white(frames[150][180]));
}

#[test]
fn edits_that_would_move_the_chart_are_refused() {
    let env = Env::new("order", "30", "2");
    let spec = env.spec_file("chart.json", &chart("Order", 2000, 4000));
    env.add_ok(&spec, &[]);
    let stderr = fail(
        &[
            "transform",
            "punch-in",
            "--project",
            &s(&env.project),
            "--start-ms",
            "5000",
            "--end-ms",
            "7000",
            "--scale",
            "1.2",
            "--out",
            &s(&env.project),
        ],
        2,
    );
    assert!(stderr.contains("overlays"), "{stderr}");
    let stderr = fail(
        &[
            "funnel",
            "attach-bridge",
            "--project",
            &s(&env.project),
            "--target-url",
            "example.com",
            "--out",
            &s(&env.project),
        ],
        2,
    );
    assert!(stderr.contains("overlays"), "{stderr}");
}

#[test]
fn a_missing_worker_is_exit_1_with_the_install_command() {
    let env = Env::new("noworker", "30", "2");
    let spec = env.spec_file("chart.json", &chart("None", 2000, 4000));
    let empty = env.dir.join("emptybin");
    fs::create_dir_all(&empty).expect("dir");
    let out = env.dir.join("never.otio");
    let mut command = reelctl();
    command
        .args(["overlay", "add-chart", "--project"])
        .arg(&env.project)
        .arg("--spec")
        .arg(&spec)
        .arg("--out")
        .arg(&out)
        .env("PATH", &empty);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("uv tool install --python 3.14 manimgx"),
        "{stderr}"
    );
    assert!(
        !out.exists(),
        "nothing is written when the worker is missing"
    );

    // MANIMGX_BIN is used, and a path that is not a program is exit 1 too.
    let mut command = reelctl();
    command
        .args(["overlay", "add-chart", "--project"])
        .arg(&env.project)
        .arg("--spec")
        .arg(&spec)
        .arg("--out")
        .arg(&out)
        .env("MANIMGX_BIN", env.dir.join("nonexistent"));
    assert_eq!(run_command(command).status.code(), Some(1));
    let mut command = reelctl();
    command
        .args(["overlay", "add-chart", "--project"])
        .arg(&env.project)
        .arg("--spec")
        .arg(&spec)
        .arg("--out")
        .arg(&out)
        .env("MANIMGX_BIN", &env.fake);
    assert!(
        run_command(command).status.success(),
        "MANIMGX_BIN names the worker"
    );
}

#[test]
fn urls_and_bad_specs_are_exit_2_before_any_worker_is_started() {
    let env = Env::new("usage", "30", "2");
    let good = env.spec_file("good.json", &chart("Good", 2000, 4000));
    let p = s(&env.project);
    for args in [
        vec![
            "overlay",
            "add-chart",
            "--project",
            &p,
            "--spec",
            "https://example.com/spec.json",
            "--out",
            &p,
        ],
        vec![
            "overlay",
            "add-chart",
            "--project",
            &p,
            "--spec",
            "file:///etc/hosts",
            "--out",
            &p,
        ],
    ] {
        assert!(fail(&args, 2).contains("URL"));
    }
    let g = s(&good);
    let f = s(&env.fake);
    assert!(fail(
        &[
            "overlay",
            "add-chart",
            "--project",
            &p,
            "--spec",
            &g,
            "--out",
            &p,
            "--manimgx",
            "https://example.com/manimgx"
        ],
        2
    )
    .contains("URL"));
    assert!(fail(
        &[
            "overlay",
            "add-chart",
            "--project",
            &p,
            "--spec",
            &g,
            "--out",
            &p,
            "--manimgx",
            &f,
            "--assets-dir",
            "http://example.com/a"
        ],
        2
    )
    .contains("URL"));

    let bad = |name: &str, spec: Value| {
        let path = env.spec_file(name, &spec);
        fail(
            &[
                "overlay",
                "add-chart",
                "--project",
                &p,
                "--spec",
                &s(&path),
                "--out",
                &p,
                "--manimgx",
                &f,
            ],
            2,
        )
    };
    let mut unknown = chart("U", 2000, 4000);
    unknown["colour"] = json!("red");
    assert!(bad("unknown.json", unknown).contains("colour"));
    assert!(bad("control.json", chart("tab\u{0007}bell", 2000, 4000)).contains("U+0007"));
    assert!(bad("outside.json", chart("Out", 9000, 11_000)).contains("timeline"));
    assert!(bad("short.json", chart("Short", 2000, 2400)).contains("500"));
    let mut kind = chart("K", 2000, 4000);
    kind["kind"] = json!("pie");
    assert!(bad("kind.json", kind).contains("pie"));

    // A spec over 64 KiB, and a directory for a spec.
    let big = env.dir.join("big.json");
    fs::write(
        &big,
        format!("{}{}", chart("Big", 2000, 4000), " ".repeat(70_000)),
    )
    .expect("big");
    assert!(fail(
        &[
            "overlay",
            "add-chart",
            "--project",
            &p,
            "--spec",
            &s(&big),
            "--out",
            &p,
            "--manimgx",
            &f
        ],
        2
    )
    .contains("at most 65536"));
    assert!(fail(
        &[
            "overlay",
            "add-chart",
            "--project",
            &p,
            "--spec",
            &s(&env.dir),
            "--out",
            &p,
            "--manimgx",
            &f
        ],
        2
    )
    .contains("not a regular file"));
    assert!(
        env.calls().is_empty(),
        "the worker was started for a bad request: {:?}",
        env.calls()
    );
}

#[test]
fn layout_problems_stop_the_chart_and_keep_the_storyboard() {
    let env = Env::new("layout", "30", "2");
    env.set("inspect.exit", "1");
    let spec = env.spec_file("chart.json", &chart("Crowded", 2000, 4000));
    let before = fs::read(&env.project).expect("project");
    let output = env.add(&spec, &env.project, &[]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layout problems") && stderr.contains("text overlaps the title"),
        "{stderr}"
    );
    assert_eq!(
        env.count("render"),
        0,
        "a chart with layout problems is not rendered"
    );
    assert_eq!(fs::read(&env.project).expect("project"), before);
    let kept: Vec<String> = fs::read_dir(env.dir.join("edit.overlays"))
        .expect("assets")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].ends_with(".inspect.png"), "{kept:?}");
    // With the check turned off the chart is drawn.
    env.add_ok(&spec, &["--skip-inspect"]);
    assert_eq!(env.count("inspect"), 1);
    assert_eq!(env.count("render"), 1);
    // Any other inspect failure is exit 1.
    let other = Env::new("layout2", "30", "2");
    other.set("inspect.exit", "2");
    let spec = other.spec_file("chart.json", &chart("Broken", 2000, 4000));
    assert_eq!(other.add(&spec, &other.project, &[]).status.code(), Some(1));
}

#[test]
fn a_worker_that_fails_or_draws_the_wrong_thing_leaves_the_project_alone() {
    let spec_of = |env: &Env| env.spec_file("chart.json", &chart("Bad", 2000, 4000));

    // The render fails.
    let env = Env::new("failrender", "30", "2");
    env.set("render.exit", "3");
    let before = fs::read(&env.project).expect("project");
    let output = env.add(&spec_of(&env), &env.project, &[]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no GPU adapter") && stderr.contains("exit 3"),
        "{stderr}"
    );
    assert_eq!(fs::read(&env.project).expect("project"), before);
    assert!(
        !env.dir.join("edit.overlays").exists()
            || fs::read_dir(env.dir.join("edit.overlays"))
                .expect("d")
                .count()
                == 0
    );

    // The video is not on the key colour.
    let env = Env::new("wrongbg", "30", "2");
    env.set("render.bg", "0x000000");
    let output = env.add(&spec_of(&env), &env.project, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("key colour"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The video is the wrong length (a 5 s chart for a 2 s span).
    let env = Env::new("wronglen", "30", "5");
    let output = env.add(&spec_of(&env), &env.project, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("within one frame"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_worker_that_does_not_finish_is_stopped() {
    let env = Env::new("slow", "30", "2");
    let slow = env.dir.join("slow");
    fs::write(
        &slow,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo slow; exit 0; fi\nsleep 60\n",
    )
    .expect("script");
    fs::set_permissions(&slow, fs::Permissions::from_mode(0o755)).expect("chmod");
    let spec = env.spec_file("chart.json", &chart("Slow", 2000, 4000));
    let started = std::time::Instant::now();
    let mut command = reelctl();
    command
        .args(["overlay", "add-chart", "--project"])
        .arg(&env.project)
        .arg("--spec")
        .arg(&spec)
        .arg("--out")
        .arg(&env.project)
        .arg("--manimgx")
        .arg(&slow)
        .args(["--timeout-s", "2"]);
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    assert!(started.elapsed().as_secs() < 30);
    assert!(String::from_utf8_lossy(&output.stderr).contains("did not finish in 2 s"));
}

#[test]
fn chart_text_never_reaches_the_workers_command_line_or_the_render() {
    let env = Env::new("hostile", "30", "2");
    // A real key in the environment must not reach the worker either; this is a
    // made-up value, set on purpose to see that it is removed.
    let title = "\"\"\" + __import__('os') + $(touch pwned) `x` ; \\ \n";
    let title = title.trim_end_matches('\n');
    let mut spec = chart(title, 2000, 4000);
    spec["series"][0]["label"] = json!("it's \"x\" \\");
    let spec = env.spec_file("chart.json", &spec);
    let mut command = reelctl();
    command
        .args(["overlay", "add-chart", "--project"])
        .arg(&env.project)
        .arg("--spec")
        .arg(&spec)
        .arg("--out")
        .arg(&env.project)
        .arg("--manimgx")
        .arg(&env.fake)
        .env("OPENROUTER_API_KEY", "not-a-real-key");
    let output = run_command(command);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let argv = fs::read_to_string(env.worker_dir().join("argv.log")).expect("log");
    for needle in ["__import__", "pwned", "it's", "Reels", "Mon", "Tue"] {
        assert!(
            !argv.contains(needle),
            "{needle} reached the command line: {argv}"
        );
    }
    let keys = fs::read_to_string(env.worker_dir().join("key.log")).expect("keys");
    assert!(keys.lines().all(|line| line == "unset"), "{keys}");
    assert!(!env.dir.join("pwned").exists() && !env.worker_dir().join("pwned").exists());

    // The scene holds the text once, in the one literal.
    let scene = fs::read_to_string(env.worker_dir().join("scene.copy.py")).expect("scene");
    assert_eq!(scene.matches("__import__").count(), 1);
    let line = scene
        .lines()
        .find(|l| l.contains("__import__"))
        .expect("line");
    assert!(line.starts_with("DATA = json.loads(\""), "{line}");

    // And the render's filter graph (and so its command line) has none of it.
    let project = fs::read_to_string(&env.project).expect("project");
    assert!(
        project.contains("__import__"),
        "the project keeps the spec as data"
    );
    let out = env.dir.join("out.mp4");
    env.render(&out);
    assert_eq!(duration_frames(&out), 300);
}

#[test]
fn a_chart_whose_video_has_gone_cannot_be_rendered() {
    let env = Env::new("gone", "30", "2");
    let spec = env.spec_file("chart.json", &chart("Gone", 2000, 4000));
    env.add_ok(&spec, &[]);
    for entry in fs::read_dir(env.dir.join("edit.overlays")).expect("assets") {
        fs::remove_file(entry.expect("entry").path()).expect("rm");
    }
    let out = env.dir.join("out.mp4");
    let stderr = fail(
        &["render", "--project", &s(&env.project), "--out", &s(&out)],
        2,
    );
    assert!(stderr.contains("cannot find the media file"), "{stderr}");
    assert!(!out.exists());
}
