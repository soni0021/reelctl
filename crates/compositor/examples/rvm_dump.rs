//! Manual check, not part of the test suite: run raw `rgb24` frames from stdin
//! through the real RVM model with the crate's own session and recurrent-state
//! handling, and write each frame's alpha to stdout as little-endian `f32`.
//! `crates/compositor/manual/rvm_reference.py` runs the same frames through the
//! official Python path and compares the two.
//!
//! ```text
//! ffmpeg -i clip.mp4 -vf scale=1280:720,format=rgb24 -f rawvideo clip.rgb
//! ORT_DYLIB_PATH=/abs/libonnxruntime.dylib cargo run --release -p compositor \
//!   --example rvm_dump -- <rvm.onnx> <width> <height> <ratio> < clip.rgb > pha.f32
//! ```

use std::io::{self, Read, Write};
use std::path::Path;

use compositor::{release_runtime, FrameRgb, MattingModel, RecurrentState, RvmModel};

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("error: {message}");
            1
        }
    };
    release_runtime();
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [weights, width, height, ratio] = args.as_slice() else {
        return Err(
            "usage: rvm_dump <rvm.onnx> <width> <height> <ratio> < clip.rgb > pha.f32".into(),
        );
    };
    let width: usize = width.parse().map_err(|e| format!("width: {e}"))?;
    let height: usize = height.parse().map_err(|e| format!("height: {e}"))?;
    let ratio: f32 = ratio.parse().map_err(|e| format!("ratio: {e}"))?;
    let mut model = RvmModel::open(Some(Path::new(weights))).map_err(|e| e.to_string())?;
    let mut frame = FrameRgb::new(width, height);
    let mut state = RecurrentState::initial();
    let mut bytes = vec![0u8; 3 * width * height];
    let mut stdin = io::stdin().lock();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    loop {
        match stdin.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.to_string()),
        }
        frame.load_rgb8(&bytes).map_err(|e| e.to_string())?;
        let output = model
            .matte(&frame, ratio, &state)
            .map_err(|e| e.to_string())?;
        for alpha in &output.pha {
            stdout
                .write_all(&alpha.to_le_bytes())
                .map_err(|e| e.to_string())?;
        }
        state = output.state;
    }
    stdout.flush().map_err(|e| e.to_string())
}
