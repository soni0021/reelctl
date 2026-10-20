# ADR-004: Report missing and substituted passes in edit_dna.json

## Status
Accepted

## Date
2026-10-18. Recorded on 2026-10-20 from [crates/edit-dna/src/lib.rs](../../crates/edit-dna/src/lib.rs) and [AGENTS.md](../../AGENTS.md).

## Context
planer.md's Pass 1 includes optical-flow zoom curves and a `.cube` LUT, which P0 does not compute, and two signals (cuts and words) come from stand-ins for the planned TransNetV2 and whisper.cpp. [CONSTRAINTS.md](../../CONSTRAINTS.md) bans unimplemented stubs such as `todo!()` and `unimplemented!()`. Without a marker, a reader of `edit_dna.json` cannot tell a signal that was measured and found empty from one that was never measured.

## Decision
- `zoom_curves` and `cube_lut` are always present as `{"status": "unimplemented", "reason": "..."}` (the `StubbedPass` type).
- Every signal from a stand-in names its source: `cut_detector` with `cut_detector_note`, `asr_backend` with `asr_note`, and `decode_note` for the decoder.
- A capability that was explicitly requested but is missing fails with a reason instead of being replaced silently (`--cut-detector transnet`, [ADR-002](ADR-002-scene-score-cut-detector.md)).

## Alternatives Considered

### Panicking stubs
- `todo!()` and `unimplemented!()` are banned by CONSTRAINTS.md.

No other alternatives were recorded.

## Consequences
- The output describes its own gaps. Consumers should check `status`, `cut_detector`, and `asr_backend` before relying on a signal.
- Implementing zoom curves or the LUT changes the shape of those fields. If files written before the change would no longer parse, bump `SCHEMA_VERSION`.
