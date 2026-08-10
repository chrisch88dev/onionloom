# Contributing

Keep changes focused, secure, and easy to review.

## Required checks

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --all-targets --no-default-features
cargo build --release --locked
cargo deny check
```

Search, key, output, or checkpoint changes also need a release search followed by `onionloom verify`.
Platform fixes need release-mode CLI smoke coverage on the affected operating system. Exercise automatic tuning, the largest CPU batch, a short CPU-only search, and saved-key verification. Test GPU search only when a compatible adapter is available.

## Performance changes

Record before and after commits, CPU, operating system, Rust version, exact commands, workers, batch size, patterns, trial length, warmup, sample count, median, and variation. Compare equivalent builds on the same machine and report regressions.

GPU changes must pass exact startup and overflow-replay tests. Report initialization, shader compilation, tuning, transfer, result recovery, CPU verification, and total search wall time. Automatic policy changes must preserve CPU fallback on unsupported or slower devices.

## Platform changes

Linux is the primary performance platform. Keep portable Rust paths building on Windows and macOS. Changes to release packaging must preserve the three tagged archives and `SHA256SUMS`. Do not weaken the CPU-only build.

## Safety

- never attach keys, checkpoints, memory dumps, or production paths
- keep unsafe Rust forbidden
- preserve overwrite protection and readback validation
- keep errors actionable and free of secret material

Use short imperative commit subjects. Contributions submitted for inclusion are licensed under the Apache License 2.0.
