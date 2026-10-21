//! Word timing with whisper.cpp, through `whisper-rs`.
//!
//! Each word comes back as its own segment: token timestamps on, `max_len`
//! 1, and splits only at word boundaries. That is whisper.cpp's documented
//! word-level timestamp mode ("Word-level timestamp (experimental)" in its
//! README). In 1.8.3, `max_len` re-splits segments only when token
//! timestamps are on, so both are set.
//!
//! Segment times alone stretch words across pauses, so each word starts at
//! the DTW timestamp of its first token instead, and ends at the next word or
//! the next measured silence gap. DTW needs the alignment-head preset of the
//! model's shape, which is read from the file header because fine-tuned
//! models such as Oriserve's Hinglish Apex do not carry a standard file name.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use edit_dna::{SilenceGap, Word};
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters,
};

use crate::error::DecompileError;
use crate::resample::to_whisper_rate;

const GGML_MAGIC: u32 = 0x6767_6d6c;

/// One whisper.cpp segment. Times are in centiseconds, as whisper.cpp reports them.
pub(crate) struct RawSegment {
    pub text: String,
    pub start_cs: i64,
    pub end_cs: i64,
    /// DTW time of the segment's first text token, when whisper.cpp gave one.
    pub onset_cs: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct Transcript {
    pub words: Vec<Word>,
    pub preset: &'static str,
    pub language: String,
}

/// The header fields that tell whisper.cpp model shapes apart.
#[derive(Debug, PartialEq)]
pub(crate) struct ModelShape {
    pub n_vocab: i32,
    pub n_audio_state: i32,
    pub n_audio_layer: i32,
    pub n_text_layer: i32,
}

pub(crate) fn transcribe(
    model: &Path,
    pcm: &[f32],
    sample_rate: u32,
    silence_gaps: &[SilenceGap],
) -> Result<Transcript, DecompileError> {
    if !model.is_file() {
        return Err(DecompileError::Whisper(format!(
            "model not found at {}",
            model.display()
        )));
    }
    let shape = read_shape(model)?;
    let (model_preset, preset) = preset_for(&shape).ok_or_else(|| {
        DecompileError::Whisper(format!(
            "model {} has a shape no DTW preset matches: {shape:?}",
            model.display()
        ))
    })?;
    if pcm.is_empty() {
        return Ok(Transcript {
            words: Vec::new(),
            preset,
            language: String::new(),
        });
    }
    let audio = to_whisper_rate(pcm, sample_rate)?;
    // With neither of whisper-rs's log-backend features enabled, this
    // discards whisper.cpp's log output instead of printing it to stderr.
    whisper_rs::install_logging_hooks();
    let mut context_params = WhisperContextParameters::default();
    // whisper.cpp computes DTW timestamps only without flash attention.
    context_params.flash_attn(false);
    context_params.dtw_parameters(DtwParameters {
        mode: DtwMode::ModelPreset { model_preset },
        ..Default::default()
    });
    let context = WhisperContext::new_with_params(model, context_params)
        .map_err(|err| failed("load", model, err))?;
    let mut state = context
        .create_state()
        .map_err(|err| failed("create a state for", model, err))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if context.is_multilingual() {
        params.set_language(None);
    }
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    params.set_suppress_nst(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state
        .full(params, &audio)
        .map_err(|err| failed("transcribe with", model, err))?;
    let special_from = context.token_eot();
    let mut segments = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|err| failed("read a segment from", model, err))?;
        let onset_cs = (0..segment.n_tokens())
            .filter_map(|index| segment.get_token(index))
            .find(|token| token.token_id() < special_from)
            .map(|token| token.token_data().t_dtw)
            .filter(|t_dtw| *t_dtw >= 0);
        segments.push(RawSegment {
            text: text.into_owned(),
            start_cs: segment.start_timestamp(),
            end_cs: segment.end_timestamp(),
            onset_cs,
        });
    }
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
        .unwrap_or("unknown")
        .to_string();
    Ok(Transcript {
        words: words_from_segments(&segments, silence_gaps),
        preset,
        language,
    })
}

fn failed(action: &str, model: &Path, err: whisper_rs::WhisperError) -> DecompileError {
    DecompileError::Whisper(format!(
        "could not {action} model {}: {err}",
        model.display()
    ))
}

/// Reads the magic and the eleven `i32` hyperparameters that open a ggml
/// whisper model (`whisper_model_load` in whisper.cpp 1.8.3).
pub(crate) fn read_shape(model: &Path) -> Result<ModelShape, DecompileError> {
    let mut header = [0u8; 48];
    File::open(model)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|err| {
            DecompileError::Whisper(format!("could not read model {}: {err}", model.display()))
        })?;
    let field = |index: usize| {
        let at = 4 + index * 4;
        i32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    if u32::from_le_bytes([header[0], header[1], header[2], header[3]]) != GGML_MAGIC {
        return Err(DecompileError::Whisper(format!(
            "{} is not a ggml whisper model",
            model.display()
        )));
    }
    Ok(ModelShape {
        n_vocab: field(0),
        n_audio_state: field(2),
        n_audio_layer: field(4),
        n_text_layer: field(8),
    })
}

/// Matches a shape to whisper.cpp's alignment-head presets. Vocabulary 51864
/// is English-only and 51866 is large-v3. large-v1 and large-v2 share a
/// shape, so both get the v2 preset.
pub(crate) fn preset_for(shape: &ModelShape) -> Option<(DtwModelPreset, &'static str)> {
    let english = shape.n_vocab == 51_864;
    let preset = match (shape.n_audio_state, shape.n_audio_layer, shape.n_text_layer) {
        (384, 4, 4) if english => (DtwModelPreset::TinyEn, "tiny.en"),
        (384, 4, 4) => (DtwModelPreset::Tiny, "tiny"),
        (512, 6, 6) if english => (DtwModelPreset::BaseEn, "base.en"),
        (512, 6, 6) => (DtwModelPreset::Base, "base"),
        (768, 12, 12) if english => (DtwModelPreset::SmallEn, "small.en"),
        (768, 12, 12) => (DtwModelPreset::Small, "small"),
        (1_024, 24, 24) if english => (DtwModelPreset::MediumEn, "medium.en"),
        (1_024, 24, 24) => (DtwModelPreset::Medium, "medium"),
        (1_280, 32, 32) if shape.n_vocab == 51_866 => (DtwModelPreset::LargeV3, "large-v3"),
        (1_280, 32, 32) if shape.n_vocab == 51_865 => (DtwModelPreset::LargeV2, "large-v2"),
        (1_280, 32, 4) if shape.n_vocab == 51_866 => {
            (DtwModelPreset::LargeV3Turbo, "large-v3-turbo")
        }
        _ => return None,
    };
    Some(preset)
}

/// Turns one-word segments into words. A word starts at its DTW onset, or at
/// the segment start without one. It ends at the next word's start, or at
/// the first silence gap that opens after it starts, whichever is earlier;
/// the last word falls back to its segment end.
pub(crate) fn words_from_segments(
    segments: &[RawSegment],
    silence_gaps: &[SilenceGap],
) -> Vec<Word> {
    let kept: Vec<(&str, u64, u64)> = segments
        .iter()
        .filter_map(|segment| {
            let text = segment.text.trim();
            if !text.chars().any(char::is_alphanumeric) || is_marker(text) {
                return None;
            }
            let start_ms = centiseconds_to_ms(segment.onset_cs.unwrap_or(segment.start_cs));
            Some((text, start_ms, centiseconds_to_ms(segment.end_cs)))
        })
        .collect();
    kept.iter()
        .enumerate()
        .map(|(index, &(text, start_ms, segment_end_ms))| {
            let next_start = kept.get(index + 1).map_or(segment_end_ms, |next| next.1);
            let gap_start = silence_gaps
                .iter()
                .map(|gap| gap.start_ms)
                .find(|&gap_start| gap_start > start_ms)
                .unwrap_or(u64::MAX);
            Word {
                text: text.to_string(),
                start_ms,
                end_ms: next_start.min(gap_start).max(start_ms),
            }
        })
        .collect()
}

/// Non-speech annotations such as `[BLANK_AUDIO]` or `(music)`.
fn is_marker(text: &str) -> bool {
    (text.starts_with('[') && text.ends_with(']')) || (text.starts_with('(') && text.ends_with(')'))
}

fn centiseconds_to_ms(centiseconds: i64) -> u64 {
    u64::try_from(centiseconds).unwrap_or(0).saturating_mul(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(text: &str, start_cs: i64, end_cs: i64) -> RawSegment {
        timed(text, start_cs, end_cs, None)
    }

    fn timed(text: &str, start_cs: i64, end_cs: i64, onset_cs: Option<i64>) -> RawSegment {
        RawSegment {
            text: text.to_string(),
            start_cs,
            end_cs,
            onset_cs,
        }
    }

    fn shape(
        n_vocab: i32,
        n_audio_state: i32,
        n_audio_layer: i32,
        n_text_layer: i32,
    ) -> ModelShape {
        ModelShape {
            n_vocab,
            n_audio_state,
            n_audio_layer,
            n_text_layer,
        }
    }

    #[test]
    fn links_the_documented_whisper_cpp_version() {
        // docs/specs/whisper-asr.md and asr_note name this version.
        assert_eq!(whisper_rs::get_whisper_version(), "1.8.3");
    }

    #[test]
    fn segments_become_trimmed_words_in_milliseconds() {
        let words = words_from_segments(&[segment(" Cut", 0, 32), segment(" the", 32, 45)], &[]);
        assert_eq!(
            words,
            vec![
                Word {
                    text: "Cut".to_string(),
                    start_ms: 0,
                    end_ms: 320,
                },
                Word {
                    text: "the".to_string(),
                    start_ms: 320,
                    end_ms: 450,
                },
            ]
        );
    }

    #[test]
    fn markers_blanks_and_bare_punctuation_are_not_words() {
        let words = words_from_segments(
            &[
                segment(" [BLANK_AUDIO]", 0, 100),
                segment(" (music)", 100, 200),
                segment(" ", 200, 210),
                segment(".", 210, 220),
                segment(" air.", 220, 260),
            ],
            &[],
        );
        assert_eq!(words.len(), 1, "{words:?}");
        assert_eq!(words[0].text, "air.");
    }

    #[test]
    fn negative_or_reversed_times_are_clamped() {
        let words = words_from_segments(&[segment(" so", -3, 20), segment(" then", 50, 40)], &[]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (0, 500));
        assert_eq!((words[1].start_ms, words[1].end_ms), (500, 500));
    }

    #[test]
    fn a_word_starts_at_its_dtw_onset_and_ends_at_the_next_word() {
        let words = words_from_segments(
            &[
                timed(" dead", 91, 200, Some(95)),
                timed(" air.", 200, 260, Some(203)),
            ],
            &[],
        );
        assert_eq!((words[0].start_ms, words[0].end_ms), (950, 2_030));
        assert_eq!((words[1].start_ms, words[1].end_ms), (2_030, 2_600));
    }

    #[test]
    fn a_word_ends_where_a_silence_gap_starts() {
        let gaps = [SilenceGap {
            start_ms: 1_200,
            end_ms: 2_000,
            duration_ms: 800,
        }];
        let words = words_from_segments(
            &[
                timed(" dead", 91, 200, Some(95)),
                timed(" air.", 200, 260, Some(203)),
                timed(" now", 270, 400, Some(275)),
            ],
            &gaps,
        );
        assert_eq!((words[0].start_ms, words[0].end_ms), (950, 1_200));
        assert_eq!((words[1].start_ms, words[1].end_ms), (2_030, 2_750));
        assert_eq!((words[2].start_ms, words[2].end_ms), (2_750, 4_000));
    }

    #[test]
    fn the_header_shape_picks_the_dtw_preset() {
        // Header values of ggml-base.en.bin and ggml-apex-hinglish-q8_0.bin.
        let base_en = shape(51_864, 512, 6, 6);
        let apex = shape(51_866, 1_280, 32, 4);
        assert_eq!(preset_for(&base_en).map(|(_, name)| name), Some("base.en"));
        assert_eq!(
            preset_for(&apex).map(|(_, name)| name),
            Some("large-v3-turbo")
        );
        assert!(preset_for(&shape(51_865, 640, 7, 7)).is_none());
    }

    #[test]
    fn a_file_without_the_ggml_magic_is_an_error_naming_the_path() {
        let path = std::env::temp_dir().join("reelctl-not-a-model.bin");
        std::fs::write(&path, [0u8; 64]).expect("write temp file");
        let err = read_shape(&path).expect_err("no magic");
        std::fs::remove_file(&path).ok();
        assert!(err.to_string().contains("reelctl-not-a-model.bin"), "{err}");
    }

    #[test]
    fn a_missing_model_is_an_error_naming_the_path() {
        let path = Path::new("/nonexistent/ggml-missing.bin");
        let err = transcribe(path, &[0.0; 4_800], 48_000, &[]).expect_err("no model file");
        assert!(
            err.to_string().contains("/nonexistent/ggml-missing.bin"),
            "{err}"
        );
    }
}
