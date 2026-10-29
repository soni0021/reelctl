//! The frame loop: raw `rgb24` frames in, raw frames out, the model between them.
//!
//! It reads and writes through `Read` and `Write`, so it runs the same over
//! pipes to `ffmpeg` and over byte slices in a test. One frame is in flight at a
//! time, and frame N+1 needs the state frame N returned, so frames are strictly
//! in order.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::background::Background;
use crate::composite::{composite_frame, matte_frame, rgba_frame, OutputMode};
use crate::error::CompositorError;
use crate::model::MattingModel;
use crate::tensor::{clamp_unit, FrameRgb, RecurrentState};

/// What a run needs to know about its frames.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub width: usize,
    pub height: usize,
    /// The downsample ratio, fixed for the whole clip.
    pub ratio: f32,
    pub mode: OutputMode,
}

/// What a run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub frames: usize,
    /// Time spent inside the model, not reading, compositing or writing.
    pub model_time: Duration,
}

/// Read until `buffer` is full. `Ok(false)` is a clean end of input (nothing
/// read); a frame cut short is an error, never padded.
fn read_frame<R: Read>(
    input: &mut R,
    buffer: &mut [u8],
    index: usize,
) -> Result<bool, CompositorError> {
    let mut filled = 0;
    while filled < buffer.len() {
        match input.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(CompositorError::run(format!(
                    "cannot read frame {index} from the decoder: {error}"
                )))
            }
        }
    }
    match filled {
        0 => Ok(false),
        n if n == buffer.len() => Ok(true),
        n => Err(CompositorError::run(format!(
            "frame {index} is {n} bytes of {} (the decoder stopped in the middle of a frame)",
            buffer.len()
        ))),
    }
}

/// Run every frame of `input` through `model` and write the result to `output`.
/// `background` is required in [`OutputMode::Composite`] and unused otherwise.
pub fn isolate<R: Read, W: Write>(
    mut input: R,
    mut background: Option<&mut dyn Background>,
    mut output: W,
    model: &mut dyn MattingModel,
    options: &Options,
) -> Result<Stats, CompositorError> {
    if options.mode == OutputMode::Composite && background.is_none() {
        return Err(CompositorError::setup(
            "composite output needs a background (--background)",
        ));
    }
    let pixels = options.width * options.height;
    let mut source = vec![0u8; 3 * pixels];
    let mut behind = vec![0u8; 3 * pixels];
    let mut result = vec![0u8; options.mode.bytes_per_pixel() * pixels];
    let mut frame = FrameRgb::new(options.width, options.height);
    let mut state = RecurrentState::initial();
    let mut frames = 0;
    let mut model_time = Duration::ZERO;

    while read_frame(&mut input, &mut source, frames)? {
        frame.load_rgb8(&source)?;
        let started = Instant::now();
        let mut matte = model
            .matte(&frame, options.ratio, &state)
            .map_err(|error| in_frame(frames, error))?;
        model_time += started.elapsed();
        if matte.fgr.len() != 3 * pixels || matte.pha.len() != pixels {
            return Err(CompositorError::run(format!(
                "the model gave {} foreground and {} alpha values for frame {frames}, \
expected {} and {pixels}",
                matte.fgr.len(),
                matte.pha.len(),
                3 * pixels
            )));
        }
        clamp_unit(&mut matte.pha, "alpha", frames)?;
        clamp_unit(&mut matte.fgr, "foreground", frames)?;
        match options.mode {
            OutputMode::Composite => {
                if let Some(background) = background.as_deref_mut() {
                    background.next_frame(&mut behind)?;
                }
                composite_frame(&matte.fgr, &matte.pha, &behind, &mut result);
            }
            OutputMode::Matte => matte_frame(&matte.pha, &mut result),
            OutputMode::Rgba => rgba_frame(&matte.fgr, &matte.pha, &mut result),
        }
        output.write_all(&result).map_err(|error| {
            CompositorError::run(format!(
                "cannot write frame {frames} to the encoder: {error}"
            ))
        })?;
        state = matte.state;
        frames += 1;
    }
    if frames == 0 {
        return Err(CompositorError::run("the video has no frames"));
    }
    output
        .flush()
        .map_err(|error| CompositorError::run(format!("cannot flush the encoder: {error}")))?;
    Ok(Stats { frames, model_time })
}

/// `error` with the frame it happened on, keeping its kind.
fn in_frame(frame: usize, error: CompositorError) -> CompositorError {
    match error {
        CompositorError::Setup(message) => {
            CompositorError::Setup(format!("frame {frame}: {message}"))
        }
        CompositorError::Run(message) => CompositorError::Run(format!("frame {frame}: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::SolidColor;
    use crate::model::ModelOutput;
    use crate::tensor::Tensor4;

    /// A model that gives the source back as the foreground with a fixed alpha, and
    /// records what it was asked. Its state is hostile: it changes shape each call
    /// and carries the call number, so what a call received says which call made it.
    struct Fake {
        alpha: f32,
        calls: Vec<Call>,
        fail_at: Option<usize>,
        bad_pha: Option<f32>,
        delay: Duration,
    }

    struct Call {
        ratio: f32,
        width: usize,
        height: usize,
        state: RecurrentState,
    }

    impl Fake {
        fn new(alpha: f32) -> Self {
            Self {
                alpha,
                calls: Vec::new(),
                fail_at: None,
                bad_pha: None,
                delay: Duration::ZERO,
            }
        }

        /// The state call `k` returns: tensor `j` is `[1, 1, 1, k + 2]` of `10k + j`.
        fn state_of(k: usize) -> RecurrentState {
            let tensor = |j: usize| Tensor4 {
                shape: [1, 1, 1, k + 2],
                data: vec![(10 * k + j) as f32; k + 2],
            };
            RecurrentState([tensor(0), tensor(1), tensor(2), tensor(3)])
        }
    }

    impl MattingModel for Fake {
        fn matte(
            &mut self,
            frame: &FrameRgb,
            ratio: f32,
            state: &RecurrentState,
        ) -> Result<ModelOutput, CompositorError> {
            let k = self.calls.len();
            self.calls.push(Call {
                ratio,
                width: frame.width,
                height: frame.height,
                state: state.clone(),
            });
            std::thread::sleep(self.delay);
            if self.fail_at == Some(k) {
                return Err(CompositorError::run("the session failed"));
            }
            let pixels = frame.width * frame.height;
            let mut pha = vec![self.alpha; pixels];
            if let Some(bad) = self.bad_pha {
                pha[0] = bad;
            }
            Ok(ModelOutput {
                fgr: frame.data.clone(),
                pha,
                state: Fake::state_of(k),
            })
        }
    }

    fn options(mode: OutputMode) -> Options {
        Options {
            width: 2,
            height: 1,
            ratio: 0.25,
            mode,
        }
    }

    /// Three 2x1 frames; frame `i` is pixels (10i, 0, 0) and (0, 10i, 0).
    fn video() -> Vec<u8> {
        (0..3u8)
            .flat_map(|i| [10 * i, 0, 0, 0, 10 * i, 0])
            .collect()
    }

    fn run(
        input: &[u8],
        model: &mut Fake,
        mode: OutputMode,
    ) -> (Result<Stats, CompositorError>, Vec<u8>) {
        let mut background = SolidColor([200, 200, 200]);
        let mut out = Vec::new();
        let result = isolate(
            input,
            Some(&mut background),
            &mut out,
            model,
            &options(mode),
        );
        (result, out)
    }

    #[test]
    fn every_frame_is_composited_in_order() {
        let mut model = Fake::new(1.0);
        let (result, out) = run(&video(), &mut model, OutputMode::Composite);
        assert_eq!(result.unwrap().frames, 3);
        // Alpha 1: the foreground (the source here) shows, background hidden.
        assert_eq!(out, video());
    }

    #[test]
    fn the_models_foreground_is_what_is_composited_not_the_source_frame() {
        // The foreground is green whatever the source is; alpha is 1.
        struct Green;
        impl MattingModel for Green {
            fn matte(
                &mut self,
                _: &FrameRgb,
                _: f32,
                _: &RecurrentState,
            ) -> Result<ModelOutput, CompositorError> {
                Ok(ModelOutput {
                    fgr: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0],
                    pha: vec![1.0, 1.0],
                    state: RecurrentState::initial(),
                })
            }
        }
        let mut background = SolidColor([7, 7, 7]);
        let mut out = Vec::new();
        isolate(
            &video()[6..12],
            Some(&mut background),
            &mut out,
            &mut Green,
            &options(OutputMode::Composite),
        )
        .unwrap();
        assert_eq!(out, vec![0, 255, 0, 0, 255, 0]);
        // And the same in the rgba mode.
        let mut out = Vec::new();
        isolate(
            &video()[6..12],
            None,
            &mut out,
            &mut Green,
            &options(OutputMode::Rgba),
        )
        .unwrap();
        assert_eq!(out, vec![0, 255, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn a_foreground_past_the_bounds_is_clamped_and_a_non_finite_one_is_an_error() {
        struct Foreground(f32);
        impl MattingModel for Foreground {
            fn matte(
                &mut self,
                _: &FrameRgb,
                _: f32,
                _: &RecurrentState,
            ) -> Result<ModelOutput, CompositorError> {
                Ok(ModelOutput {
                    fgr: vec![self.0; 6],
                    pha: vec![0.5, 0.5],
                    state: RecurrentState::initial(),
                })
            }
        }
        let run = |value: f32| {
            let mut background = SolidColor([100, 100, 100]);
            let mut out = Vec::new();
            let result = isolate(
                &video()[6..12],
                Some(&mut background),
                &mut out,
                &mut Foreground(value),
                &options(OutputMode::Composite),
            );
            (result, out)
        };
        // 1.4 counts as 1: half of 255 plus half of 100 is 177.5, not 0.5*1.4*255 + 50.
        let (result, out) = run(1.4);
        result.unwrap();
        assert_eq!(out, vec![178; 6]);
        // -0.4 counts as 0.
        let (result, out) = run(-0.4);
        result.unwrap();
        assert_eq!(out, vec![50; 6]);
        let error = run(f32::NAN).0.unwrap_err();
        assert!(
            error.to_string().contains("non-finite foreground"),
            "{error}"
        );
    }

    #[test]
    fn alpha_zero_shows_the_background() {
        let mut model = Fake::new(0.0);
        let (_, out) = run(&video(), &mut model, OutputMode::Composite);
        assert_eq!(out, vec![200; 18]);
    }

    #[test]
    fn frame_n_receives_the_state_frame_n_minus_1_returned() {
        let mut model = Fake::new(1.0);
        let (result, _) = run(&video(), &mut model, OutputMode::Composite);
        result.unwrap();
        assert_eq!(model.calls.len(), 3);
        assert_eq!(model.calls[0].state, RecurrentState::initial());
        assert_eq!(model.calls[1].state, Fake::state_of(0));
        assert_eq!(model.calls[2].state, Fake::state_of(1));
        // And the later states are not the zeros of the first.
        assert_ne!(model.calls[2].state, RecurrentState::initial());
    }

    #[test]
    fn the_ratio_and_frame_size_reach_the_model_on_every_call() {
        let mut model = Fake::new(1.0);
        run(&video(), &mut model, OutputMode::Composite).0.unwrap();
        for call in &model.calls {
            assert_eq!(call.ratio, 0.25);
            assert_eq!((call.width, call.height), (2, 1));
        }
    }

    #[test]
    fn the_model_time_is_counted() {
        let mut model = Fake::new(1.0);
        model.delay = Duration::from_millis(20);
        let stats = run(&video(), &mut model, OutputMode::Composite).0.unwrap();
        assert_eq!(stats.frames, 3);
        // Three calls of 20 ms each are all inside the model's time.
        assert!(
            stats.model_time >= Duration::from_millis(60),
            "{:?}",
            stats.model_time
        );
    }

    #[test]
    fn a_frame_cut_short_is_an_error_not_padded() {
        let mut input = video();
        input.truncate(input.len() - 2);
        let mut model = Fake::new(1.0);
        let (result, _) = run(&input, &mut model, OutputMode::Composite);
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("frame 2 is 4 bytes of 6"),
            "{error}"
        );
        assert!(!error.is_setup());
    }

    #[test]
    fn an_empty_stream_is_an_error() {
        let mut model = Fake::new(1.0);
        let (result, _) = run(&[], &mut model, OutputMode::Composite);
        assert!(result.unwrap_err().to_string().contains("no frames"));
        assert!(model.calls.is_empty());
    }

    #[test]
    fn a_model_error_stops_the_run_and_names_the_frame() {
        let mut model = Fake::new(1.0);
        model.fail_at = Some(1);
        let (result, out) = run(&video(), &mut model, OutputMode::Composite);
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("frame 1: the session failed"),
            "{error}"
        );
        assert_eq!(model.calls.len(), 2);
        // Frame 0 had been written; nothing after the failure.
        assert_eq!(out.len(), 6);
    }

    #[test]
    fn a_non_finite_alpha_is_an_error() {
        for bad in [f32::NAN, f32::INFINITY] {
            let mut model = Fake::new(1.0);
            model.bad_pha = Some(bad);
            let (result, _) = run(&video(), &mut model, OutputMode::Composite);
            assert!(result.unwrap_err().to_string().contains("non-finite alpha"));
        }
    }

    #[test]
    fn an_alpha_past_the_bounds_is_clamped() {
        let mut model = Fake::new(1.0);
        model.bad_pha = Some(1.4);
        // Frame 1 is (10, 0, 0) and (0, 10, 0). Alpha 1.4 becomes 1 on pixel 0:
        // the foreground shows, not an overshoot past the background.
        let (result, out) = run(&video()[6..12], &mut model, OutputMode::Composite);
        result.unwrap();
        assert_eq!(out, vec![10, 0, 0, 0, 10, 0]);
        let mut model = Fake::new(1.0);
        model.bad_pha = Some(-0.3);
        // Alpha -0.3 becomes 0: pixel 0 is the background.
        let (_, out) = run(&video()[6..12], &mut model, OutputMode::Composite);
        assert_eq!(out, vec![200, 200, 200, 0, 10, 0]);
    }

    #[test]
    fn a_model_that_returns_the_wrong_number_of_values_is_refused() {
        struct Short;
        impl MattingModel for Short {
            fn matte(
                &mut self,
                _: &FrameRgb,
                _: f32,
                _: &RecurrentState,
            ) -> Result<ModelOutput, CompositorError> {
                Ok(ModelOutput {
                    fgr: vec![0.0; 6],
                    pha: vec![0.0; 3],
                    state: RecurrentState::initial(),
                })
            }
        }
        let mut out = Vec::new();
        let mut background = SolidColor([0, 0, 0]);
        let error = isolate(
            &video()[..6],
            Some(&mut background),
            &mut out,
            &mut Short,
            &options(OutputMode::Composite),
        )
        .unwrap_err();
        assert!(error.to_string().contains("3 alpha values"), "{error}");
    }

    #[test]
    fn composite_without_a_background_is_a_setup_error() {
        let mut model = Fake::new(1.0);
        let mut out = Vec::new();
        let error = isolate(
            &video()[..],
            None,
            &mut out,
            &mut model,
            &options(OutputMode::Composite),
        )
        .unwrap_err();
        assert!(error.is_setup());
        assert!(model.calls.is_empty());
    }

    #[test]
    fn the_matte_mode_writes_one_byte_per_pixel_and_needs_no_background() {
        let mut model = Fake::new(0.5);
        let mut out = Vec::new();
        let stats = isolate(
            &video()[..],
            None,
            &mut out,
            &mut model,
            &options(OutputMode::Matte),
        )
        .unwrap();
        assert_eq!(stats.frames, 3);
        assert_eq!(out, vec![128; 6]);
    }

    #[test]
    fn the_rgba_mode_writes_four_bytes_per_pixel() {
        let mut model = Fake::new(0.5);
        let mut out = Vec::new();
        isolate(
            &video()[6..12],
            None,
            &mut out,
            &mut model,
            &options(OutputMode::Rgba),
        )
        .unwrap();
        // Frame 1 is (10, 0, 0) and (0, 10, 0); alpha 0.5 is 128.
        assert_eq!(out, vec![10, 0, 0, 128, 0, 10, 0, 128]);
    }

    #[test]
    fn a_background_frame_is_taken_once_per_frame() {
        struct Counting(usize);
        impl Background for Counting {
            fn next_frame(&mut self, out: &mut [u8]) -> Result<(), CompositorError> {
                self.0 += 1;
                out.fill(self.0 as u8);
                Ok(())
            }
        }
        let mut model = Fake::new(0.0);
        let mut background = Counting(0);
        let mut out = Vec::new();
        isolate(
            &video()[..],
            Some(&mut background),
            &mut out,
            &mut model,
            &options(OutputMode::Composite),
        )
        .unwrap();
        assert_eq!(background.0, 3);
        assert_eq!(out, [vec![1u8; 6], vec![2; 6], vec![3; 6]].concat());
    }

    #[test]
    fn a_write_that_fails_is_an_error() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut model = Fake::new(1.0);
        let mut background = SolidColor([0, 0, 0]);
        let error = isolate(
            &video()[..],
            Some(&mut background),
            Broken,
            &mut model,
            &options(OutputMode::Composite),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("cannot write frame 0"),
            "{error}"
        );
    }

    #[test]
    fn a_reader_that_returns_small_pieces_is_still_read_in_whole_frames() {
        struct Trickle<'a>(&'a [u8]);
        impl Read for Trickle<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let count = self.0.len().min(buffer.len()).min(4);
                buffer[..count].copy_from_slice(&self.0[..count]);
                self.0 = &self.0[count..];
                Ok(count)
            }
        }
        let bytes = video();
        let mut model = Fake::new(1.0);
        let mut background = SolidColor([0, 0, 0]);
        let mut out = Vec::new();
        let stats = isolate(
            Trickle(&bytes),
            Some(&mut background),
            &mut out,
            &mut model,
            &options(OutputMode::Composite),
        )
        .unwrap();
        assert_eq!(stats.frames, 3);
        assert_eq!(out, bytes);
    }
}
