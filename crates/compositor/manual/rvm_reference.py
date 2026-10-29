"""Manual check: RobustVideoMatting through the official ONNX Runtime Python path,
compared with the crate's own session (`cargo run -p compositor --example rvm_dump`).

Not part of the test suite (it needs the weights, ONNX Runtime and numpy). Run with
`python3 -I` from a directory that holds no other Python files:

    python3 -I rvm_reference.py <rvm.onnx> <width> <height> <ratio> <clip.rgb> <pha.f32>

`clip.rgb` is raw rgb24 frames (the same bytes given to `rvm_dump`) and `pha.f32` is
what `rvm_dump` wrote. The script runs the model the way RVM's documentation does
(recurrent states fed back, `downsample_ratio` as a float32 array) and prints the
largest and mean absolute difference of the alpha, per frame range.
"""
import sys

import numpy as np
import onnxruntime as ort


def main() -> None:
    weights, width, height, ratio, clip, rust = sys.argv[1:7]
    width, height, ratio = int(width), int(height), float(ratio)
    session = ort.InferenceSession(weights, providers=["CPUExecutionProvider"])
    half = session.get_inputs()[0].type == "tensor(float16)"
    dtype = np.float16 if half else np.float32
    frame_bytes = 3 * width * height
    data = np.fromfile(clip, dtype=np.uint8)
    frames = data.size // frame_bytes
    rust_alpha = np.fromfile(rust, dtype="<f4").reshape(-1, height, width)
    if rust_alpha.shape[0] != frames:
        sys.exit(f"{frames} input frames but {rust_alpha.shape[0]} alpha frames from Rust")
    rec = [np.zeros([1, 1, 1, 1], dtype=dtype)] * 4
    ratio_in = np.array([ratio], dtype=np.float32)
    worst = []
    for index in range(frames):
        rgb = data[index * frame_bytes:(index + 1) * frame_bytes]
        src = rgb.reshape(height, width, 3).astype(np.float32) / 255.0
        src = np.ascontiguousarray(src.transpose(2, 0, 1)[None]).astype(dtype)
        fgr, pha, *rec = session.run(
            [],
            {"src": src, "r1i": rec[0], "r2i": rec[1], "r3i": rec[2], "r4i": rec[3],
             "downsample_ratio": ratio_in},
        )
        difference = np.abs(pha[0, 0].astype(np.float32) - rust_alpha[index])
        worst.append((float(difference.max()), float(difference.mean())))
    maxima = np.array([w[0] for w in worst])
    means = np.array([w[1] for w in worst])
    print(f"frames={frames} precision={'float16' if half else 'float32'}")
    print(f"max_abs_diff over all frames: {maxima.max():.6f} (frame {int(maxima.argmax())})")
    print(f"mean_abs_diff over all frames: {means.mean():.8f}")
    print(f"first frame max {maxima[0]:.6f}, last frame max {maxima[-1]:.6f}")


if __name__ == "__main__":
    main()
