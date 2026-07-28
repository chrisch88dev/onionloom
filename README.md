# Onionloom

Onionloom is a CPU-first Rust CLI for generating Tor v3 onion vanity addresses. It uses batched Ed25519 point compression, measures the current machine before a search, and writes key directories in Tor's native format.

The project is early but usable. The current backend is CPU only. There is no GPU backend and no GPU performance claim.

## Why Onionloom

- Pure Rust search and key handling with unsafe code forbidden
- Automatic worker and batch-size tuning against the requested filter
- Prefix matching directly against compressed public-key bits
- Live measured throughput with expected, median, and 95 percent time estimates
- Exact Tor tagged public and expanded secret-key formats
- Operating-system randomness and in-memory zeroization
- New-output-only writes, readback validation, and atomic directory installation
- Opt-in resumable work with an explicit secret-material warning
- Linux, macOS, and Windows source compatibility

## Install

Rust 1.85 or newer is required.

```console
cargo install --path .
```

For a machine-specific release build:

```console
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

The native CPU flag is suitable for a binary used on the build machine. Do not use it for portable release artifacts.

## Search

```console
onionloom search loom
```

Generated directories go under `onions/` by default:

```text
onions/
└── loom...onion/
    ├── hostname
    ├── hs_ed25519_public_key
    └── hs_ed25519_secret_key
```

Useful options:

```console
onionloom search loom onion -d ./keys -n 2
onionloom search loom --workers 8 --batch-size 512
onionloom search loom --checkpoint /secure/path/loom.checkpoint
onionloom verify ./keys/loom...onion
onionloom benchmark --trial-ms 500
```

Patterns use lowercase `a-z` and digits `2-7`. Onionloom currently accepts prefix patterns up to 51 characters. This limit keeps matching in the fast public-key-only path and keeps the probability model exact.

The default tuner tests five batch sizes and every available logical worker count. Supplying `--workers` or `--batch-size` narrows that work. The measured rate includes the selected pattern filter. Very short trials are more sensitive to scheduler noise, so use a larger `--tune-ms` when stable estimates matter more than startup time.

## Reading the time estimates

Vanity generation is probabilistic. A displayed median is not a deadline.

- Expected time is the arithmetic mean for the next match.
- Median time is when the chance of having found a match reaches 50 percent.
- The 95 percent estimate is when the chance reaches 95 percent.

The estimates use a geometric distribution, the exact combined probability of the normalized non-overlapping prefixes, and measured candidate throughput from the current run.

## Key safety

On Unix, Onionloom creates key directories with mode `0700` and all key files with mode `0600`. It refuses to overwrite an existing onion directory. It recomputes the public key from the expanded secret, validates the onion checksum and requested prefix, writes to a temporary directory, reads every file back, and only then renames the directory into place.

Windows output inherits the access-control list of the chosen parent directory. Use a directory restricted to the account running Onionloom. Native Windows ACL hardening is tracked as a release blocker before a stable 1.0 claim.

A checkpoint contains the random expanded base secret and is equivalent to sensitive key material. Checkpoints are opt-in, written with mode `0600` on Unix, and removed after successful completion. Put them on encrypted local storage. Never sync them to cloud drives or commit them.

Read [SECURITY.md](SECURITY.md) before using generated keys for an important service.

## How the CPU engine works

Each worker starts from a securely generated expanded Ed25519 scalar. Candidate scalars advance by eight, which preserves Ed25519 clamp bits. The corresponding public points advance by `8B`, avoiding a full base-point multiplication per candidate. Workers generate disjoint ranges and compress whole batches with one field inversion. Prefix filters compare packed bits without base32 allocation or checksum hashing in the hot loop.

A found candidate is treated as untrusted until the scalar offset is applied to the expanded secret and its public key is independently recomputed.

More detail is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Measured development snapshot

This is a local engineering measurement, not a universal speed claim.

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| CPU | Intel Core i5-12450HX, 12 logical CPUs |
| OS | Fedora Linux, kernel 7.1.4, x86_64 |
| Rust | 1.95.0 |
| Profile | `release`, thin LTO, one codegen unit |
| Command | `onionloom benchmark --trial-ms 500` |
| Selected | 12 workers, batch 1024 |
| Candidate engine rate | 19.53 million candidates per second |

The benchmark measures point generation and batch compression. Search startup measurements additionally include the real compiled filter and are the values used for ETA. No same-machine mkp224o result is published yet because its build prerequisites were not present in the measurement environment.

Benchmark policy and reproducibility requirements are in [docs/BENCHMARKING.md](docs/BENCHMARKING.md).

## Project status

Implemented:

- CPU batch engine
- exact prefix filters and probability model
- automatic worker and batch tuning
- progress and ETA reporting
- Tor key serialization and address validation
- atomic guarded output
- opt-in resumable checkpoints
- verification command

Before 1.0:

- independent Tor daemon interoperability fixtures
- stronger Windows ACL setup and validation
- cross-platform release packaging and signing
- fuzzing for parsers and checkpoint handling
- long-duration same-machine comparisons against current mkp224o
- external cryptographic review

GPU exploration comes after the CPU baseline is stable. A GPU backend will only become a normal feature after repeatable end-to-end benchmarks prove a useful win on real prefix searches.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md). Performance changes need before-and-after results, correctness changes need vectors or regression tests, and public claims need reproducible evidence.

Onionloom is licensed under the [MIT License](LICENSE).
