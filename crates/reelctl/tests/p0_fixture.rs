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
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("OPENROUTER_API_KEY");
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

/// A recording whose sound is all quieter than the old -40 dB floor used to be
/// one silence gap from start to end. Its pause is the only gap of 180 ms or
/// more (the mute is switched per 1,024-sample audio frame, so it starts
/// 13 ms late), and `diff` flags that pause as dead air.
#[test]
fn a_pause_in_a_very_quiet_recording_is_the_only_long_gap() {
    let root = std::env::temp_dir().join(format!("reelctl-quiet-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let mp4 = root.join("quiet.mp4");
    generate("quiet12", &mp4);
    let (dna, _) = decompile_with(&root, &mp4, "quiet", "auto");

    assert!(
        dna.silence_threshold_db < -40.0,
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
        start.abs_diff(5_000) <= 40 && end.abs_diff(5_500) <= 40,
        "pause {long:?}"
    );

    let json = root.join("quiet.edit_dna.json");
    let diff = reelctl()
        .args(["diff", "--draft"])
        .arg(&mp4)
        .arg("--outlier")
        .arg(&json)
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

#[test]
fn ten_bit_hlg_video_reads_like_the_eight_bit_draft() {
    let root = std::env::temp_dir().join(format!("reelctl-10bit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let decompile = |kind: &str| -> EditDna {
        let mp4 = root.join(format!("{kind}.mp4"));
        let json = root.join(format!("{kind}.edit_dna.json"));
        generate(kind, &mp4);
        let output = reelctl()
            .args(["decompile", "--input"])
            .arg(&mp4)
            .arg("--out")
            .arg(&json)
            .arg("--filmstrip-dir")
            .arg(root.join(format!("{kind}_timeline")))
            .output()
            .expect("spawn reelctl");
        assert!(
            output.status.success(),
            "decompile {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&fs::read(&json).expect("json")).expect("dna")
    };
    let eight = decompile("draft12");
    let ten = decompile("draft12_10bit");

    assert_eq!(ten.cuts.len(), eight.cuts.len(), "cuts {:?}", ten.cuts);
    for (a, b) in ten.cuts.iter().zip(&eight.cuts) {
        assert!(a.frame_index.abs_diff(b.frame_index) <= 1, "{a:?} vs {b:?}");
    }
    assert_eq!(ten.silence_gaps, eight.silence_gaps);
    assert_eq!(ten.payoff_window.has_cue, eight.payoff_window.has_cue);
    assert_eq!(ten.shot_histograms.len(), eight.shot_histograms.len());
    for (a, b) in ten.shot_histograms.iter().zip(&eight.shot_histograms) {
        for (x, y) in [
            (a.mean_r, b.mean_r),
            (a.mean_g, b.mean_g),
            (a.mean_b, b.mean_b),
        ] {
            assert!(
                (x - y).abs() <= 3.0,
                "shot {}: {a:?} vs {b:?}",
                a.shot_index
            );
        }
    }
    assert!(ten.decode_note.contains("10-bit"), "{}", ten.decode_note);
    assert!(ten.decode_note.contains("HLG"), "{}", ten.decode_note);
    assert!(
        !eight.decode_note.contains("10-bit"),
        "{}",
        eight.decode_note
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_pause_at_50_seconds_in_44k_audio_is_found_on_time() {
    let root = std::env::temp_dir().join(format!("reelctl-44k-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let mp4 = root.join("draft_44k.mp4");
    let json = root.join("draft_44k.edit_dna.json");
    generate("draft12_44k", &mp4);

    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&mp4)
        .arg("--out")
        .arg(&json)
        .arg("--filmstrip-dir")
        .arg(root.join("timeline"))
        .output()
        .expect("spawn reelctl");
    assert!(
        output.status.success(),
        "decompile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dna: EditDna = serde_json::from_slice(&fs::read(&json).expect("json")).expect("dna");
    let long: Vec<(u64, u64)> = dna
        .silence_gaps
        .iter()
        .filter(|gap| gap.duration_ms >= 180)
        .map(|gap| (gap.start_ms, gap.end_ms))
        .collect();
    assert_eq!(long.len(), 1, "gaps of 180 ms or more: {long:?}");
    let (start, end) = long[0];
    // ffmpeg's `volume enable=` switches on 1,024-sample frame boundaries, so
    // the mute is at 50.0159-50.5034 s (checked on the source signal). Per-frame
    // resampling found 50,000-50,470, 16 and 33 ms early.
    assert!(
        start.abs_diff(50_016) <= 10 && end.abs_diff(50_503) <= 10,
        "pause {long:?}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn hard_cuts_between_textured_shots_without_keyframes_are_all_found() {
    let root = std::env::temp_dir().join(format!("reelctl-textured-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let mp4 = root.join("textured.mp4");
    let json = root.join("textured.edit_dna.json");
    generate("textured_cuts12", &mp4);

    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&mp4)
        .arg("--out")
        .arg(&json)
        .arg("--filmstrip-dir")
        .arg(root.join("timeline"))
        .output()
        .expect("spawn reelctl");
    assert!(
        output.status.success(),
        "decompile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dna: EditDna = serde_json::from_slice(&fs::read(&json).expect("json")).expect("dna");

    assert_no_keyframe_on_the_cuts(&mp4);

    // Four 3 s shots at 24 fps, so cuts at frames 72, 144, and 216. The encoder
    // has scene-cut detection off and keyframes every 25 frames.
    let found: Vec<u64> = dna.cuts.iter().map(|cut| cut.frame_index).collect();
    assert_eq!(found.len(), 3, "cuts {:?}", dna.cuts);
    for (cut, planted) in found.iter().zip([72u64, 144, 216]) {
        assert!(cut.abs_diff(planted) <= 1, "cuts {found:?}");
    }
    assert_eq!(dna.shot_histograms.len(), 4);

    let _ = fs::remove_dir_all(&root);
}

/// The fixture is only worth having if the encoder put no keyframe within a
/// frame of a cut at frames 72, 144, or 216, so check the file's own packets.
fn assert_no_keyframe_on_the_cuts(mp4: &std::path::Path) {
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"])
        .arg(mp4)
        .output()
        .expect("spawn ffprobe");
    assert!(probe.status.success(), "ffprobe failed");
    let keyframes: Vec<u64> = String::from_utf8_lossy(&probe.stdout)
        .lines()
        .filter_map(|line| {
            let (time, flags) = line.split_once(',')?;
            let frame = (time.parse::<f64>().ok()? * 24.0).round() as u64;
            flags.contains('K').then_some(frame)
        })
        .collect();
    assert!(keyframes.len() >= 10, "keyframes {keyframes:?}");
    for planted in [72u64, 144, 216] {
        assert!(
            keyframes.iter().all(|key| key.abs_diff(planted) > 1),
            "a keyframe sits on the cut at frame {planted}: {keyframes:?}"
        );
    }
}

/// Decompile `mp4` with the given `--decoder`; returns the parsed output and
/// the directory holding its timeline PNGs.
fn decompile_with(
    root: &std::path::Path,
    mp4: &std::path::Path,
    name: &str,
    decoder: &str,
) -> (EditDna, PathBuf) {
    let json = root.join(format!("{name}.edit_dna.json"));
    let views = root.join(format!("{name}_timeline"));
    let output = reelctl()
        .args(["decompile", "--decoder", decoder, "--input"])
        .arg(mp4)
        .arg("--out")
        .arg(&json)
        .arg("--filmstrip-dir")
        .arg(&views)
        .output()
        .expect("spawn reelctl");
    assert!(
        output.status.success(),
        "decompile --decoder {decoder} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dna = serde_json::from_slice(&fs::read(&json).expect("json")).expect("dna");
    (dna, views)
}

fn pngs(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
        .expect("timeline dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("png"),
            )
        })
        .collect();
    files.sort();
    files
}

/// HEVC goes to the media engine. The results must equal the software
/// decoder's, and the note must say which decoder ran: a silent fallback would
/// make the two equal by construction.
#[test]
fn hevc_on_the_media_engine_gives_the_same_results_as_software() {
    let root = std::env::temp_dir().join(format!("reelctl-hw-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let on_mac = cfg!(target_os = "macos");

    for kind in [
        "draft12_10bit",
        "textured_cuts12_hevc",
        "textured_cuts12_hevc8",
        "textured_cuts12_hevc_full",
    ] {
        let mp4 = root.join(format!("{kind}.mp4"));
        generate(kind, &mp4);
        if kind.starts_with("textured") {
            assert_no_keyframe_on_the_cuts(&mp4);
        }
        let (auto, auto_views) = decompile_with(&root, &mp4, &format!("{kind}_auto"), "auto");
        let (soft, soft_views) = decompile_with(&root, &mp4, &format!("{kind}_soft"), "software");

        assert!(
            soft.decode_note.contains("--decoder software"),
            "{kind}: {}",
            soft.decode_note
        );
        if on_mac {
            assert!(
                auto.decode_note.contains("Apple media engine"),
                "{kind}: the hardware path did not run: {}",
                auto.decode_note
            );
        } else {
            assert!(!auto.decode_note.contains("Apple media engine"), "{kind}");
        }

        if kind.starts_with("textured") {
            let found: Vec<u64> = auto.cuts.iter().map(|cut| cut.frame_index).collect();
            assert_eq!(found.len(), 3, "{kind}: {found:?}");
            for (frame, planted) in found.iter().zip([72u64, 144, 216]) {
                assert!(frame.abs_diff(planted) <= 1, "{kind}: {found:?}");
            }
        }
        assert_eq!(auto.cuts.len(), soft.cuts.len(), "{kind}: {:?}", auto.cuts);
        for (a, s) in auto.cuts.iter().zip(&soft.cuts) {
            assert_eq!(a.frame_index, s.frame_index, "{kind}");
            assert_eq!(a.time_ms, s.time_ms, "{kind}");
            // Both decoders filter every frame, so the pictures are the same.
            assert!(
                (a.scene_score - s.scene_score).abs() <= 0.0001,
                "{kind}: {a:?} vs {s:?}"
            );
        }
        assert_eq!(auto.frame_count, soft.frame_count, "{kind}");
        assert_eq!(auto.duration_ms, soft.duration_ms, "{kind}");
        assert_eq!(auto.fps, soft.fps, "{kind}");
        assert_eq!(auto.frame_size, soft.frame_size, "{kind}");
        assert_eq!(auto.silence_gaps, soft.silence_gaps, "{kind}");
        assert_eq!(auto.shot_histograms, soft.shot_histograms, "{kind}");
        assert_eq!(auto.payoff_window, soft.payoff_window, "{kind}");
        // The 10-bit and HDR sentences come from the first frame's metadata,
        // so they must survive the copy out of the media engine.
        for sentence in ["10-bit", "HLG"] {
            assert_eq!(
                auto.decode_note.contains(sentence),
                soft.decode_note.contains(sentence),
                "{kind}: {sentence}"
            );
        }
        assert_eq!(
            pngs(&auto_views),
            pngs(&soft_views),
            "{kind}: thumbnails differ"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

/// A stream that changes size is one the media engine's frame copy can get
/// wrong without an error: the copy reuses its buffers, which are the old size.
/// The software decoder reads the new frames, so the media engine must too.
#[test]
fn a_hevc_stream_that_changes_size_decodes_the_same_on_both_decoders() {
    let root = std::env::temp_dir().join(format!("reelctl-size-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");

    for (kind, last_width) in [("resize12_hevc", 640), ("shrink12_hevc", 320)] {
        let hevc = root.join(format!("{kind}.hevc"));
        generate(kind, &hevc);

        let (auto, auto_views) = decompile_with(&root, &hevc, &format!("{kind}_auto"), "auto");
        let (soft, soft_views) = decompile_with(&root, &hevc, &format!("{kind}_soft"), "software");

        if cfg!(target_os = "macos") {
            assert!(
                auto.decode_note.contains("Apple media engine"),
                "{kind}: the hardware path did not run: {}",
                auto.decode_note
            );
        }
        assert_eq!(soft.frame_size, auto.frame_size, "{kind}");
        assert_eq!(auto.frame_size.width, last_width, "{kind}");
        assert_eq!(auto.frame_count, soft.frame_count, "{kind}");
        assert_eq!(auto.cuts.len(), soft.cuts.len(), "{kind}: {:?}", auto.cuts);
        for (a, s) in auto.cuts.iter().zip(&soft.cuts) {
            assert_eq!(a.frame_index, s.frame_index, "{kind}");
            assert!(
                (a.scene_score - s.scene_score).abs() <= 0.0001,
                "{kind}: {a:?} vs {s:?}"
            );
        }
        assert_eq!(auto.shot_histograms, soft.shot_histograms, "{kind}");
        assert_eq!(
            pngs(&auto_views),
            pngs(&soft_views),
            "{kind}: thumbnails differ"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

/// A stream that changes from 8-bit to 10-bit part-way through needs its frame
/// copy remade in the new pixel format. The media engine must decode it, with
/// the same results as software.
#[test]
fn a_hevc_stream_that_changes_bit_depth_decodes_the_same_on_both_decoders() {
    let root = std::env::temp_dir().join(format!("reelctl-depth-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    let hevc = root.join("depth12_hevc.hevc");
    generate("depth12_hevc", &hevc);

    let (auto, auto_views) = decompile_with(&root, &hevc, "auto", "auto");
    let (soft, soft_views) = decompile_with(&root, &hevc, "soft", "software");

    if cfg!(target_os = "macos") {
        assert!(
            auto.decode_note.contains("Apple media engine"),
            "the hardware path did not run: {}",
            auto.decode_note
        );
    }
    assert_eq!(auto.frame_size, soft.frame_size);
    assert_eq!(auto.frame_count, soft.frame_count);
    assert_eq!(auto.cuts.len(), soft.cuts.len(), "{:?}", auto.cuts);
    assert_eq!(auto.shot_histograms, soft.shot_histograms);
    assert_eq!(pngs(&auto_views), pngs(&soft_views), "thumbnails differ");

    let _ = fs::remove_dir_all(&root);
}

/// `reelctl diff` with the given extra arguments, on fixtures `draft12` and the
/// listed outliers (decompiled first). Returns the output.
fn diff_fixtures(
    root: &std::path::Path,
    outliers: &[&str],
    extra: &[&str],
) -> std::process::Output {
    let draft = root.join("draft.mp4");
    if !draft.exists() {
        generate("draft12", &draft);
    }
    let mut command = reelctl();
    command.args(["diff", "--draft"]).arg(&draft);
    for kind in outliers {
        let mp4 = root.join(format!("{kind}.mp4"));
        let json = root.join(format!("{kind}.edit_dna.json"));
        if !json.exists() {
            generate(kind, &mp4);
            let _ = decompile_with(root, &mp4, kind, "auto");
        }
        command.arg("--outlier").arg(&json);
    }
    command.args(extra);
    command.output().expect("spawn diff")
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("reelctl-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    root
}

#[test]
fn diff_with_one_outlier_keeps_its_flags_and_adds_a_comparison() {
    let root = scratch("cmp1");
    let output = diff_fixtures(&root, &["outlier12"], &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");

    // The flags are what `diff` printed before this feature.
    let kinds: Vec<&str> = report["flags"]
        .as_array()
        .expect("flags")
        .iter()
        .filter_map(|flag| flag["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "dead_air",
            "static_window",
            "static_window",
            "missing_payoff_cue"
        ]
    );

    let comparison = &report["comparison"];
    assert_eq!(comparison["draft"]["name"], "draft");
    assert_eq!(comparison["outliers"][0]["name"], "outlier12");
    assert_eq!(comparison["draft"]["cut_count"], 1);
    assert_eq!(comparison["outliers"][0]["cut_count"], 2);
    // The draft cuts at 6 s; the outlier at 3 s and 9 s.
    // The draft also pauses 5 times a minute (one 0.5 s mute in 12 s) where the
    // outlier never does.
    let found = comparison["flags"].as_array().expect("relative flags");
    let kinds: Vec<&str> = found.iter().filter_map(|f| f["kind"].as_str()).collect();
    assert_eq!(kinds, ["rhythm_gap", "pause_density"], "{found:?}");
    assert_eq!(found[0]["window_start_ms"], 0);
    assert_eq!(found[0]["window_end_ms"], 5_000);
    assert_eq!(found[1]["draft_per_minute"], 5.0);
    assert_eq!(found[1]["outlier_per_minute"], 0.0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn diff_with_two_outliers_compares_each_and_votes() {
    let root = scratch("cmp2");
    let output = diff_fixtures(&root, &["outlier12", "textured_cuts12"], &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let comparison = &report["comparison"];
    assert_eq!(
        comparison["outliers"].as_array().expect("outliers").len(),
        2
    );
    assert_eq!(comparison["outliers"][1]["name"], "textured_cuts12");
    assert_eq!(comparison["outliers"][1]["cut_count"], 3);
    // 0-5 s: both outliers cut (at 3 s), the draft does not. 5-10 s: the draft cuts.
    let windows = comparison["windows"].as_array().expect("windows");
    assert_eq!(windows[0]["draft_cuts"], 0);
    assert_eq!(windows[0]["outlier_cuts"], serde_json::json!([1, 1]));
    assert_eq!(windows[1]["draft_cuts"], 1);
    let found = comparison["flags"].as_array().expect("relative flags");
    let kinds: Vec<&str> = found.iter().filter_map(|f| f["kind"].as_str()).collect();
    assert_eq!(kinds, ["rhythm_gap", "pause_density"], "{found:?}");
    assert_eq!(found[0]["outliers_with_cut"], 2);
    assert_eq!(found[0]["outliers_counted"], 2);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn diff_with_four_outliers_is_an_argument_error_before_any_decoding() {
    let root = scratch("cmp4");
    let output = diff_fixtures(&root, &["outlier12"], &[]);
    assert!(output.status.success());
    let json = root.join("outlier12.edit_dna.json");
    // The draft does not exist: if the count were checked after decoding it,
    // the error would be about the draft and the exit code 1.
    let mut command = reelctl();
    command
        .args(["diff", "--draft"])
        .arg(root.join("no-such-draft.mp4"));
    for _ in 0..4 {
        command.arg("--outlier").arg(&json);
    }
    let output = command.output().expect("spawn diff");
    assert_eq!(output.status.code(), Some(2), "exit code");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("at most three"), "{stderr}");
    assert!(stderr.contains("--draft"), "the usage is diff's: {stderr}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn diff_without_an_outlier_is_an_argument_error() {
    let root = scratch("cmp0");
    let draft = root.join("draft.mp4");
    generate("draft12", &draft);
    let output = reelctl()
        .args(["diff", "--draft"])
        .arg(&draft)
        .output()
        .expect("spawn diff");
    assert_eq!(output.status.code(), Some(2), "exit code");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn diff_can_print_a_text_report_and_json_is_the_default() {
    let root = scratch("cmpt");
    let text = diff_fixtures(&root, &["outlier12"], &["--format", "text"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    let header = text.lines().next().expect("header");
    assert!(
        header.contains("draft") && header.contains("outlier12"),
        "{text}"
    );
    assert!(text.contains("cuts per 5 s window"), "{text}");
    assert!(
        text.contains("NO CUT 0.0-5.0 s: 1 of 1 outliers cut here, the draft does not"),
        "{text}"
    );
    assert!(text.contains("DEAD AIR 2.01-2.51 s (500 ms)"), "{text}");
    assert!(text.contains("NO PAYOFF 8-10 s"), "{text}");

    let json = diff_fixtures(&root, &["outlier12"], &["--format", "json"]);
    assert!(serde_json::from_slice::<serde_json::Value>(&json.stdout).is_ok());
    let default = diff_fixtures(&root, &["outlier12"], &[]);
    assert_eq!(json.stdout, default.stdout, "json is the default");
    let _ = fs::remove_dir_all(&root);
}
