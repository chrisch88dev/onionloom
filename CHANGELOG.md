# Changelog

## 1.0.1 -- 2026-08-10

### Fixed

- Moved large Ed25519 offset-table construction buffers to the heap, fixing the immediate `STATUS_STACK_OVERFLOW` failure in CPU-backed commands on Windows x86_64 ([#8](https://github.com/chrisch88dev/onionloom/issues/8)).
- Preserved exact offset ordering and batch compression while replacing eight fixed-array builder variants with one size-checked runtime builder.
- Restored compatibility with the advertised Rust 1.85 minimum by removing syntax stabilized after that compiler release.

### Validation

- Added a regression test that constructs and checks the largest offset table on a 1 MiB thread stack.
- Added release-mode CLI smoke tests for evaluation, estimation, maximum-batch CPU measurement, search, and saved-key verification on Linux, Windows, Apple Silicon macOS, and Intel macOS.
- Added a Rust 1.85 minimum-version build check and both GPU-enabled and CPU-only tests across supported operating systems.

### Performance

- Automatic tuning now builds the largest immutable offset table once and shares its exact prefixes across every measured batch size instead of rebuilding common points.

### Distribution

- Release archives now include the executable, license, notice, README, changelog, security policy, contribution guide, and project documentation.
- Clarified portable and host-optimized build instructions, GPU runtime requirements, benchmark discipline, and cross-platform invocation.

## 1.0.0 -- 2026-07-31

First public release.

### Core

- Paired-coordinate CPU engine with shared offset tables and simultaneous field division.
- Exact y-only prefiltering with y-coordinate compressed candidate enumeration.
- Independent CPU verification of every generated key through `curve25519-dalek`.

### GPU

- Automatic Vulkan (Linux), DirectX 12 / Vulkan (Windows), and Metal (macOS) discovery.
- Exact arithmetic self-test and per-adapter measurement before GPU is enabled.
- CPU-verified prehit recovery for every GPU-returned offset.
- Lossless overflow replay with workgroup-level splitting.
- Unsupported or unhelpful adapters fall back to CPU automatically.

### CLI

- Command-first interface: `search`, `evaluate`, `guess`, `benchmark`, `gpu`, `verify`.
- Power modes: `--power low|medium|full` with automatic worker and batch tuning.
- GPU control: `--gpu auto|off|force`.
- Continuous search: `--continuous`.
- Opt-in resumable checkpoints: `--checkpoint`.
- Live progress with expected, median, and 95th percentile time estimates.

### Output

- Exact Tor tagged key format with readback validation.
- Atomic directory installation with overwrite protection.
- Whole-directory key verification with `onionloom verify`.

### Platform

- Linux-first with Linux x86_64, Windows x86_64, and universal macOS release artifacts.
- Linux host-optimized release script (`build.sh`): target-cpu=native, LTO, stripped.
- Tagged release packaging with SHA-256 checksums.
