# ADR-020: Build the watchtower as a separate Go module over recorded data

## Status
Accepted

## Date
2026-10-28

## Context
planer.md's P1 is a Go watchtower on Virlo's API: outlier multiplier, growth velocity, and a digest of why a creator is growing. There is no Virlo key in the repo and its response format is not documented here, so a live client cannot be built or tested.

## Decision
- The watchtower is a Go module in `watchtower/`, outside the Cargo workspace, standard library only. It calls the `reelctl` binary as a process (`reelctl decompile`) and shares no types with the Rust code.
- Data comes through a `Source` interface. The first and only implementation reads recorded JSON files in a documented format. A `VirloSource` is added when a key and a real recorded response are available.
- The digest reports only what `edit_dna.json` measures, with the counts behind each number: it does not claim captions or layouts.

## Consequences
- Nothing has run on live data; the numbers are checked against hand-made and recorded data and synthetic clips.
- Paths and post ids from the data files are untrusted: they are validated (no escape from the data directory, unique ids) before they reach `reelctl` or a file name.
- A burst rate from a run without a whisper model must not be reported as a word rate.
