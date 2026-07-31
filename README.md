# Onionloom

<p align="center">
  <strong>The fastest Tor v3 onion vanity address generator.</strong><br>
  CPU-first, GPU-accelerated, automatic.
</p>

<p align="center">
  <a href="https://crates.io/crates/onionloom"><img src="https://img.shields.io/crates/v/onionloom?label=crates.io&color=orange" alt="crates.io"></a>
  <a href="https://github.com/chrisch88dev/onionloom/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="license"></a>
  <a href="https://github.com/chrisch88dev/onionloom/releases"><img src="https://img.shields.io/github/v/release/chrisch88dev/onionloom?label=release" alt="release"></a>
</p>

---

Onionloom measures your machine, picks the best CPU configuration, validates compatible GPU hardware, and generates custom `.onion` addresses at full speed. Every result is independently verified before it touches disk. No network, no config files.

## What are .onion addresses?

`.onion` addresses are Tor v3 onion services, the modern standard for anonymous websites and services on the Tor network. Instead of `something.com`, you get an address like `loom4skeblahblahblahblahblahblahblahblah.onion` that routes all traffic through Tor, hiding both the server and the visitor.

Onionloom generates the cryptographic key pairs needed to run an onion service. Put the generated files in your Tor daemon's `HiddenServiceDir` and your site is live. You can run anything: a web server, an XMPP chat, a file share, a Bitcoin node.

And with Onionloom, the beginning of that address says whatever you want.

## Why Onionloom

- **Paired-coordinate CPU engine**: one field inversion shared across thousands of candidates. Extremely efficient.
- **Automatic GPU acceleration**: Vulkan, DirectX 12, or Metal. GPU results are independently verified on CPU before any key is saved.
- **No unsafe Rust**: zero `unsafe` blocks in the entire codebase.
- **Live throughput**: expected, median, and 95th percentile time estimates updated in real time.
- **Exact Tor format**: tagged public key, expanded secret, and hostname with full readback validation.
- **Never overwrites**: existing key directories are refused, never replaced.
- **OS randomness**: all secrets from the operating system CSPRNG, zeroized after use.
- **Resumable checkpoints**: stop and resume long searches (opt-in, secret material warning).
- **Linux-first**: full Windows and macOS support.

## Install

### Pre-built binaries

Download from [GitHub Releases](https://github.com/chrisch88dev/onionloom/releases). Just download and run:

| Platform | File |
|----------|------|
| Linux x86_64 | `onionloom-v1.0.0-linux-x86_64` |
| macOS (Apple Silicon + Intel) | `onionloom-v1.0.0-macos-universal` |
| Windows x86_64 | `onionloom-v1.0.0-windows-x86_64.exe` |

These are generic builds compatible with any machine. Verify with the published `SHA256SUMS`.

### Build from source (faster)

Compiling yourself targets your exact CPU, significantly faster than the generic binaries:

```
git clone https://github.com/chrisch88dev/onionloom
cd onionloom
./build.sh
```

`build.sh` enables native CPU instructions, full LTO, and symbol stripping. Linux only.

On macOS or Windows, or CPU-only:

```
cargo build --release --locked
cargo build --release --locked --no-default-features   # CPU-only
```

The `--locked` flag pins exact dependency versions from `Cargo.lock`. This gives you the same tested build every time. Always use it for reproducible, reliable binaries.

### Cargo

```
cargo install onionloom --locked
```

Requires Rust 1.85 or newer.

## Quick Start

```
./onionloom evaluate          # measure this machine
./onionloom guess loom        # estimate without generating
./onionloom search loom       # generate a key (output: onions/)
./onionloom search loom -d ./keys --count 2
./onionloom search loom --continuous -d ./keys
./onionloom search longname --gpu off
./onionloom search longname --gpu force
./onionloom search loom --checkpoint /secure/path/loom.checkpoint
./onionloom verify            # validate all keys in onions/
./onionloom verify ./keys/loom...onion
./onionloom benchmark --power full --trial-ms 500
./onionloom gpu               # inspect GPU acceleration
```

### Power modes

Control how much CPU Onionloom uses. Default is `full`:

- `low`: leave most CPU free for other work
- `medium`: balance search and responsiveness
- `full`: all available cores, no overclocking

### Using your key

Generated keys land in the `onions/` directory (or wherever `-d` points). Each match gets its own folder named after the `.onion` address:

```
onions/loom4skeblahblahblah.onion/
  hostname              # your .onion address
  hs_ed25519_public_key # public key (Tor tagged format)
  hs_ed25519_secret_key # secret key (keep this safe)
```

To run a Tor onion service, point your Tor daemon at the key directory. In `torrc`:

```
HiddenServiceDir /path/to/onions/loom4skeblahblahblah.onion
HiddenServicePort 80 127.0.0.1:8080
```

Restart Tor and your site is live at that `.onion` address.

## GPU Support

Onionloom detects compatible GPUs automatically. Every GPU result is independently verified on the CPU before a key is saved. If no compatible GPU is found, search falls back to CPU.

Requirements: 64-bit integer compute shaders, 128 or more workgroup lanes, sufficient workgroup storage.

| Platform | Backend | Compatible hardware |
|----------|---------|---------------------|
| Linux | Vulkan | NVIDIA (GTX 900+), AMD (GCN 2+), Intel Arc |
| Windows | DirectX 12 or Vulkan | NVIDIA (GTX 900+), AMD (GCN 2+), Intel Arc |
| macOS | Metal | Apple Silicon (M1+), AMD, Intel |

Use `--gpu off` for CPU-only or `--gpu force` to require GPU.

## Security

Onionloom uses `curve25519-dalek` for all cryptographic operations. Every generated key is independently verified before output. Secret material is zeroized. Keys follow Tor's exact format with readback validation.

To report a vulnerability:

- [GitHub private vulnerability reporting](https://github.com/chrisch88dev/onionloom/security/advisories/new)
- Email: [chris@chrisch88.dev](mailto:chris@chrisch88.dev) (PGP: `946C571F7423FF1E565EE5DFC9C0617647EC61F9`)

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md).

## Links

- [crates.io](https://crates.io/crates/onionloom)
- [GitHub Releases](https://github.com/chrisch88dev/onionloom/releases)
- [Architecture](docs/ARCHITECTURE.md)
- [Benchmarking](docs/BENCHMARKING.md)

## License

[Apache License 2.0](LICENSE). Copyright (c) 2026 chrisch88dev and Onionloom contributors.
