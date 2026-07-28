# Contributing

Onionloom accepts focused changes that improve correctness, security, measured performance, portability, or user experience.

## Before opening a change

For a bug, add a regression test that fails for the old behavior. For a performance change, collect a same-machine baseline before editing. For a format change, cite the Tor specification or Tor source behavior it implements.

Keep pull requests narrow enough to review. Separate formatting sweeps, dependency changes, new features, and performance work when they do not depend on each other.

## Required checks

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo build --release
```

Changes to search, serialization, output, or checkpoints also need a release-mode search smoke test followed by `onionloom verify`.

## Performance evidence

Performance pull requests must include:

- before and after commit IDs
- CPU model, logical CPU count, operating system, and power mode
- Rust version and relevant compiler flags
- exact commands
- worker and batch settings
- trial duration, warmup, sample count, median, and variation
- whether output and filter costs are included

Do not compare a native-tuned build with a portable build. Do not compare different thermal or power conditions. Report regressions as well as gains.

## Security and key hygiene

Never attach generated secret keys, checkpoints, memory dumps, or production paths. Use temporary keys made for the test and delete them after validation. Do not weaken overwrite protection, readback checks, or permission checks to make a test easier.

Unsafe Rust is forbidden in this repository. A proposal to change that boundary requires a design discussion, a smaller safe alternative analysis, isolated code, platform tests, and independent review.

## Writing and code quality

- Use direct technical language.
- Do not use em dash characters.
- Remove placeholder prose, copied boilerplate that does not fit, stale comments, and redundant headings.
- Explain invariants and non-obvious tradeoffs, not syntax.
- Do not add abstractions before there are concrete callers.
- Never present planned work as implemented.
- Never present an estimate as a measured result.
- Keep error messages actionable and free of secret material.

## Commits

Use short imperative commit subjects. Explain why in the body when the diff does not make the reason obvious. Do not add tool signatures or unrelated development narration.

By contributing, you agree that your work is licensed under the MIT License.
