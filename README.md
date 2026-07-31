# Onionloom

The fastest Tor v3 onion vanity address generator. CPU-first, GPU-accelerated, automatic.

Onionloom measures your machine, picks the best CPU configuration, validates compatible GPU hardware, and generates custom `.onion` addresses at full speed. Every result is independently verified before it touches disk. No network, no config files, no surprises.

## Why Onionloom

- Paired-coordinate CPU engine shares one field inversion across thousands of candidates
- Automatic Vulkan, DirectX 12, or Metal GPU acceleration with CPU-verified results
- No `unsafe` Rust anywhere
- Live throughput reporting with expected, median, and 95th percentile estimates
- Exact Tor v3 key format output (tagged public, expanded secret, hostname)
- OS randomness with in-memory zeroization of secrets
- Never overwrites existing keys
- Resumable checkpoints for long searches (opt-in, secret material warning)
- Linux-first with full Windows and macOS support

## GPU Support

Onionloom detects and uses compatible GPUs automatically. Every GPU result is independently verified on the CPU before a key is saved. If no compatible GPU is found, search falls back to CPU without any setup.

GPU requirements: 64-bit integer compute shaders, 128 or more workgroup lanes, and sufficient workgroup storage.

| Platform | Backend | Compatible hardware |
|----------|---------|---------------------|
| Linux | Vulkan | NVIDIA (GTX 900+), AMD (GCN 2+), Intel Arc |
| Windows | DirectX 12 or Vulkan | NVIDIA (GTX 900+), AMD (GCN 2+), Intel Arc |
| macOS | Metal | Apple Silicon (M1+), AMD, Intel |

Use `./onionloom gpu` to inspect what your machine supports. Use `--gpu off` to stay CPU-only, or `--gpu force` to require GPU.

## Install

### Cargo (all platforms)

```
cargo install onionloom --locked
```

Requires Rust 1.85 or newer.

### Build from source

```
git clone https://github.com/chrisch88dev/onionloom
cd onionloom
./build.sh          # Linux: host-optimized with target-cpu=native, LTO, stripped
```

On macOS or Windows:

```
cargo build --release --locked
```

For a CPU-only build without GPU dependencies:

```
cargo build --release --locked --no-default-features
```

`build.sh` is Linux-specific. It produces the fastest possible binary for your exact CPU by enabling native instructions, full LTO, and symbol stripping.

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

Control CPU usage with `--power`:

- `low` -- leave most CPU free for other work
- `medium` -- balance search speed and responsiveness
- `full` -- all available cores (default, no overclocking)

## Security

Onionloom uses battle-tested cryptographic libraries (`curve25519-dalek`). Every generated key is independently verified before output. Keys follow Tor's exact tagged format with readback validation. Secret material is zeroized after use.

To report a vulnerability, use [GitHub private vulnerability reporting](https://github.com/chrisch88dev/onionloom/security/advisories/new).

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md). Performance claims need benchmarks. Security issues use private reporting.

## License

[Apache License 2.0](LICENSE). Copyright (c) 2026 chrisch88dev and Onionloom contributors.
