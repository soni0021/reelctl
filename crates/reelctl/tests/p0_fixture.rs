use std::fs;
use std::path::PathBuf;
use std::process::Command;

use edit_dna::EditDna;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model and weight paths a shell profile may export,
/// so the sine-tone fixtures never run through whisper or TransNetV2.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS");
    command
}

fn generate(kind: &str, path: &std::path::Path) {
    let script = repo_root().join("fixtures/generate.sh");
    let output = Command::new("bash")
        .arg(&script)
        .arg(kind)
        .arg(path)
        .output()
        .expect("spawn fixture generator");
    assert!(
        output.status.success(),
        "fixture {kind} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn decompile_and_diff_match_the_planted_cut_silence_and_static_shot() {
    let root = std::env::temp_dir().join(format!("reelctl-p0-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let draft_mp4 = root.join("draft.mp4");
    let outlier_mp4 = root.join("outlier.mp4");
    let draft_json = root.join("draft.edit_dna.json");
    let outlier_json = root.join("outlier.edit_dna.json");
    let draft_views = root.join("draft_timeline");
    let outlier_views = root.join("outlier_timeline");
    generate("draft12", &draft_mp4);
    generate("outlier12", &outlier_mp4);

    let decompile = |input: &std::path::Path, out: &std::path::Path, views: &std::path::Path| {
        let output = reelctl()
            .args([
                "decompile",
                "--input",
                &input.to_string_lossy(),
                "--out",
                &out.to_string_lossy(),
                "--filmstrip-dir",
                &views.to_string_lossy(),
            ])
            .output()
            .expect("spawn reelctl");
        assert!(
            output.status.success(),
            "decompile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8")
    };

    let draft_line = decompile(&draft_mp4, &draft_json, &draft_views);
    let outlier_line = decompile(&outlier_mp4, &outlier_json, &outlier_views);
    assert!(draft_line.contains("signal_ms="), "{draft_line}");
    assert!(
        draft_line.contains("structural_under_500ms="),
        "{draft_line}"
    );
    assert!(outlier_line.contains("asr_ms="), "{outlier_line}");

    let draft: EditDna = serde_json::from_reader(fs::File::open(&draft_json).expect("draft json"))
        .expect("draft dna");
    let outlier: EditDna =
        serde_json::from_reader(fs::File::open(&outlier_json).expect("outlier json"))
            .expect("outlier dna");

    assert_eq!(draft.cuts.len(), 1, "draft cuts {:?}", draft.cuts);
    assert!(
        (5_800..=6_200).contains(&draft.cuts[0].time_ms),
        "cut at {:?}",
        draft.cuts[0]
    );
    assert!(
        draft
            .silence_gaps
            .iter()
            .any(|gap| { gap.duration_ms > 180 && gap.start_ms < 2_500 && gap.end_ms > 2_000 }),
        "silence {:?}",
        draft.silence_gaps
    );
    // A constant tone: the threshold is capped 12 dB below it, at about -33 dB.
    assert!(
        (-33.5..=-32.5).contains(&draft.silence_threshold_db),
        "threshold {}",
        draft.silence_threshold_db
    );
    let floor = draft.noise_floor_db.expect("draft has audio");
    assert!((-22.0..=-20.5).contains(&floor), "noise floor {floor}");
    assert!(!draft.payoff_window.has_cue);
    assert_eq!(draft.shot_histograms.len(), draft.cuts.len() + 1);
    assert!(draft.shot_histograms[0].mean_r > draft.shot_histograms[0].mean_b);
    assert!(draft.words_per_minute.is_finite());
    assert!(draft.asr_note.starts_with("No whisper model was given"));
    assert_eq!(draft.zoom_curves.status.as_str(), "unimplemented");
    assert!(draft.cube_lut.reason.contains(".cube"));
    assert!(draft.signal_ms > 0 || draft_line.contains("signal_ms="));

    assert_eq!(outlier.cuts.len(), 2, "outlier cuts {:?}", outlier.cuts);
    assert!(
        outlier.payoff_window.has_cue,
        "payoff {:?}",
        outlier.payoff_window
    );
    let cue = outlier.payoff_window.cue_ms.expect("cue");
    assert!((8_800..=9_200).contains(&cue), "cue {cue}");

    let pngs = |dir: &std::path::Path| -> usize {
        fs::read_dir(dir)
            .expect("timeline dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
            .count()
    };
    assert_eq!(pngs(&draft_views), 1);
    assert_eq!(pngs(&outlier_views), 2);
    let png = fs::read_dir(&draft_views)
        .expect("dir")
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .expect("png");
    let bytes = fs::read(png.path()).expect("png bytes");
    assert!(bytes.starts_with(b"\x89PNG"));

    let diff = reelctl()
        .args([
            "diff",
            "--draft",
            &draft_mp4.to_string_lossy(),
            "--outlier",
            &outlier_json.to_string_lossy(),
        ])
        .output()
        .expect("spawn diff");
    assert!(
        diff.status.success(),
        "diff failed: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&diff.stdout).expect("diff json");
    let flags = report["flags"].as_array().expect("flags");
    assert!(!flags.is_empty());
    let kinds: Vec<&str> = flags
        .iter()
        .filter_map(|flag| flag["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"dead_air"), "{kinds:?}");
    assert!(kinds.contains(&"static_window"), "{kinds:?}");
    assert!(kinds.contains(&"missing_payoff_cue"), "{kinds:?}");
    assert!(flags.iter().any(|flag| {
        flag["kind"] == "dead_air" && flag["duration_ms"].as_u64().unwrap_or(0) > 180
    }));
    assert!(flags.iter().any(|flag| {
        flag["kind"] == "static_window" && flag["duration_ms"].as_u64().unwrap_or(0) > 2_500
    }));

    // An outlier written before schema version 2 has neither silence field.
    let mut old: serde_json::Value =
        serde_json::from_slice(&fs::read(&outlier_json).expect("outlier json")).expect("parse");
    let object = old.as_object_mut().expect("object");
    object.remove("silence_threshold_db");
    object.remove("noise_floor_db");
    object.insert("schema_version".to_string(), 1.into());
    let old_json = root.join("outlier.v1.edit_dna.json");
    fs::write(&old_json, serde_json::to_vec(&old).expect("serialize")).expect("write v1");
    let old_diff = reelctl()
        .args([
            "diff",
            "--draft",
            &draft_mp4.to_string_lossy(),
            "--outlier",
            &old_json.to_string_lossy(),
        ])
        .output()
        .expect("spawn diff");
    assert!(
        old_diff.status.success(),
        "diff with a version 1 outlier failed: {}",
        String::from_utf8_lossy(&old_diff.stderr)
    );
    let old_report: serde_json::Value =
        serde_json::from_slice(&old_diff.stdout).expect("diff json");
    assert_eq!(old_report["flags"], report["flags"]);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_pause_under_room_noise_is_a_gap_and_dead_air() {
    let root = std::env::temp_dir().join(format!("reelctl-noisy-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let noisy_mp4 = root.join("noisy.mp4");
    let noisy_json = root.join("noisy.edit_dna.json");
    generate("noisy12", &noisy_mp4);

    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&noisy_mp4)
        .arg("--out")
        .arg(&noisy_json)
        .arg("--filmstrip-dir")
        .arg(root.join("noisy_timeline"))
        .output()
        .expect("spawn reelctl");
    assert!(
        output.status.success(),
        "decompile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dna: EditDna =
        serde_json::from_slice(&fs::read(&noisy_json).expect("noisy json")).expect("noisy dna");
    assert!(
        dna.silence_threshold_db > -40.0,
        "threshold {}",
        dna.silence_threshold_db
    );
    let long: Vec<(u64, u64)> = dna
        .silence_gaps
        .iter()
        .filter(|gap| gap.duration_ms >= 180)
        .map(|gap| (gap.start_ms, gap.end_ms))
        .collect();
    assert_eq!(long.len(), 1, "gaps of 180 ms or more: {long:?}");
    let (start, end) = long[0];
    assert!(
        start.abs_diff(2_000) <= 20 && end.abs_diff(2_500) <= 20,
        "pause {long:?}"
    );

    let diff = reelctl()
        .args(["diff", "--draft"])
        .arg(&noisy_mp4)
        .arg("--outlier")
        .arg(&noisy_json)
        .output()
        .expect("spawn diff");
    assert!(
        diff.status.success(),
        "diff failed: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&diff.stdout).expect("diff json");
    let dead_air: Vec<&serde_json::Value> = report["flags"]
        .as_array()
        .expect("flags")
        .iter()
        .filter(|flag| flag["kind"] == "dead_air")
        .collect();
    assert_eq!(dead_air.len(), 1, "{dead_air:?}");
    assert_eq!(dead_air[0]["start_ms"].as_u64(), Some(start));

    let _ = fs::remove_dir_all(&root);
}
