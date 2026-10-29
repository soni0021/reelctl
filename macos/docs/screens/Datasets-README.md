# Datasets and Eval

Screen 7 of [the app spec](../../../../../docs/specs/mac-app.md).

Files:

- `DatasetsScreen.swift`: `DatasetsScreen`, and `DatasetBuildNotice`: `dataset build` is not a service job (its inputs are folders), so the screen shows its command line from `docs/cli.md` as text with a copy button. Nothing is executed.
- `EvalViews.swift`: the eval form (an `eval` subject; truth from an `understanding` subject's head `out`, an imported `understanding` asset, or an imported `jsonl` dataset file; answers from a `jsonl` or `other` asset; the DNA, through `UnderstandDNAPicker`, when the truth is an `understanding.json`; the tolerances for a dataset file) and the report table with a filter.
- `EvalSession.swift`: submits `eval` (inputs `truth`, `answers`, optional `dna`; args `cut_tolerance_ms`, `event_tolerance_ms`), waits, reads the stdout output role `report` as `JSONValue`, and flattens it (`EvalMetrics.flatten`: dotted keys for objects, `key[i]` for arrays, keys sorted).

`--split` is not offered: a single `.jsonl` file is scored whole, and a dataset folder cannot be an input until the service takes folder inputs.

Shared code needs:

- Uses `UnderstandDNAPicker` and `UnderstandDNA.resolve` from `../Understand/UnderstandInputs.swift`.
- A `dataset.build` job in the service's catalogue (with folder inputs) would let this screen run it; then `eval` could also take the built folder as truth with `split`.

Not compiled here (no Swift toolchain in the container).
