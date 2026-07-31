# Changelog

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

- Linux-first with full Windows and macOS builds.
- Linux host-optimized release script (`build.sh`): target-cpu=native, LTO, stripped.
- Tagged release packaging with SHA-256 checksums.
