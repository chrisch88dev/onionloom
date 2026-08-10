# Onionloom

<p align="center">
  <strong>Tor v3 onion vanity address generation.</strong><br>
  Measured CPU tuning, validated GPU acceleration, verified key output.
</p>

<p align="center">
  <a href="https://crates.io/crates/onionloom"><img src="https://img.shields.io/crates/v/onionloom?label=crates.io&color=orange" alt="crates.io"></a>
  <a href="https://github.com/chrisch88dev/onionloom/actions/workflows/ci.yml"><img src="https://github.com/chrisch88dev/onionloom/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/chrisch88dev/onionloom/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="license"></a>
  <a href="https://github.com/chrisch88dev/onionloom/releases"><img src="https://img.shields.io/github/v/release/chrisch88dev/onionloom?label=release" alt="release"></a>
</p>

Onionloom generates Tor v3 Onion Service addresses with chosen prefixes. It measures supported CPU configurations on the current machine and enables a GPU only after a device self-test and local measurement. Every match is reconstructed on the CPU, then the installed key directory is read back and validated. Searching is local and requires no network connection or configuration file.

Onionloom is listed in the [Tor Project Onionmine alternatives guide](https://gitlab.torproject.org/tpo/onion-services/onionmine/-/blob/main/docs/alternatives.md). That guide acknowledges available implementations and does not imply endorsement.

## What is an onion address?

A Tor v3 Onion Service hostname contains 56 base32 characters followed by `.onion`. It is derived from the service's Ed25519 public key and routes clients to the service through Tor without a public server address.

Onionloom searches valid key pairs until the hostname begins with the text you requested. The generated directory contains the exact key files Tor expects for `HiddenServiceDir`.

## Why Onionloom

- **Paired-coordinate CPU engine**: shares one field inversion across a measured candidate batch.
- **Validated GPU acceleration**: supports Vulkan, DirectX 12, and Metal through `wgpu`, with CPU verification of every GPU result.
- **Safe Rust policy**: Onionloom's crate forbids unsafe code.
- **Measured estimates**: reports average, 50 percent, and 95 percent completion estimates from the current hardware and pattern filter.
- **Exact Tor output**: validates headers, lengths, key relationships, hostname checksum, and final directory contents.
- **Overwrite protection**: refuses to replace an existing key directory.
- **Operating-system randomness**: generates seeds through the OS and zeroizes secret buffers.
- **Resumable searches**: optional checkpoints preserve CPU search progress and are clearly treated as secret material.
- **Platform releases**: publishes Linux x86_64, Windows x86_64, and universal macOS artifacts.

## Install

### Release archives

Download the archive for your platform from [GitHub Releases](https://github.com/chrisch88dev/onionloom/releases):

| Platform | Release archive |
|----------|-----------------|
| Linux x86_64 | `onionloom-vX.Y.Z-linux-x86_64.tar.gz` |
| macOS, Apple Silicon and Intel | `onionloom-vX.Y.Z-macos-universal.tar.gz` |
| Windows x86_64 | `onionloom-vX.Y.Z-windows-x86_64.zip` |

Each archive includes the executable, project documentation, changelog, security policy, contribution guide, license, project notice, adapted-work notice, dependency license bundle, and Rust standard-library notices from the build toolchain. Verify the archive against the release's `SHA256SUMS` before extracting it. These portable builds do not use host-specific CPU flags, so choose the archive matching your operating system and architecture.

Extract the archive, enter its directory, and run `./onionloom help` on Linux or macOS. In Windows PowerShell, run `./onionloom.exe help`.

### Host-optimized Linux build

```console
git clone https://github.com/chrisch88dev/onionloom
cd onionloom
./build.sh
```

`build.sh` creates `./onionloom` with `target-cpu=native`, full LTO, path remapping, and symbol stripping. The result is tuned for the build machine and may not run on an older or different CPU. Use `./build.sh --cpu-only` to omit GPU dependencies.

On macOS and Windows, use Cargo directly:

```console
cargo build --release --locked
cargo build --release --locked --no-default-features
```

The `--locked` option uses the dependency versions in `Cargo.lock`. Compiler, linker, operating-system, and host settings can still change binary output and performance.

### Cargo install

```console
cargo install onionloom --locked
```

Rust 1.85 or newer is required.

## Quick start

```console
./onionloom evaluate
./onionloom guess loom
./onionloom search loom
./onionloom search loom -d ./keys --count 2
./onionloom search loom --continuous -d ./keys
./onionloom search longname --gpu off
./onionloom search longname --gpu force
./onionloom search loom --checkpoint /secure/path/loom.checkpoint
./onionloom verify
./onionloom verify ./keys/<56-character-hostname>.onion
./onionloom benchmark --power full --trial-ms 500
./onionloom gpu
```

Examples use a Unix shell. In Windows PowerShell, invoke `./onionloom.exe` from the extracted release directory.

Search stops after one safely saved key by default. Use `--count N` for a fixed number or `--continuous` to keep saving matches until Ctrl-C.

### Power modes

The default is `full`:

- `low`: leaves most CPU capacity available for other work
- `medium`: balances search work and responsiveness
- `full`: uses all available CPU processing slots without changing clock or voltage settings

## Use the generated key

Each match gets its own directory under `onions/`, or under the directory passed with `--output`:

```text
onions/<56-character-hostname>.onion/
  hostname
  hs_ed25519_public_key
  hs_ed25519_secret_key
```

Keep `hs_ed25519_secret_key` private. Point Tor at the directory in `torrc`:

```text
HiddenServiceDir /path/to/onions/<56-character-hostname>.onion
HiddenServicePort 80 127.0.0.1:8080
```

Restart Tor after updating its configuration. Run `onionloom verify` before installing or moving an existing key directory.

## GPU support

Onionloom checks feature limits, runs exact arithmetic test vectors, and measures each usable adapter before automatic search enables it. Unsupported, invalid, or unhelpful adapters fall back to the tuned CPU engine. A device model name alone is not enough to determine support, so use `onionloom gpu` for the result on a specific machine.

| Platform | Candidate backend |
|----------|-------------------|
| Linux | Vulkan |
| Windows | DirectX 12 or Vulkan |
| macOS | Metal |

The compute engine requires exact 64-bit integer shaders, at least 128 workgroup lanes, and sufficient workgroup storage. Use `--gpu off` for CPU-only search or `--gpu force` to require a compatible GPU.

## Performance

`onionloom benchmark` reports complete CPU candidate generation on the current machine. `onionloom gpu` reports complete GPU candidate generation after initialization. Results vary with hardware, drivers, cooling, power policy, compiler flags, and workload.

Use the reproducible comparison protocol in [docs/BENCHMARKING.md](docs/BENCHMARKING.md) before making speed or ranking claims. Architecture details are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Security

The search engine uses safe Rust field arithmetic. Every possible match is reconstructed with `curve25519-dalek` before output, and the complete installed directory is validated again from disk. Secret material is zeroized where supported and is never intentionally printed.

A recognizable prefix is not proof of identity. Verify the complete 56-character hostname through a trusted channel. See the [Tor Project vanity address guidance](https://community.torproject.org/onion-services/advanced/vanity-addresses/).

To report a vulnerability:

- [GitHub private vulnerability reporting](https://github.com/chrisch88dev/onionloom/security/advisories/new)
- [chris@chrisch88.dev](mailto:chris@chrisch88.dev), PGP fingerprint `946C 571F 7423 FF1E 565E E5DF C9C0 6176 47EC 61F9`

See [SECURITY.md](SECURITY.md) for the security model and limits.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a change.

## Links

- [crates.io](https://crates.io/crates/onionloom)
- [GitHub Releases](https://github.com/chrisch88dev/onionloom/releases)
- [Tor Project Onionmine alternatives](https://gitlab.torproject.org/tpo/onion-services/onionmine/-/blob/main/docs/alternatives.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Benchmarking](docs/BENCHMARKING.md)

## License

[Apache License 2.0](LICENSE). Copyright (c) 2026 chrisch88dev and Onionloom contributors. Required attribution for adapted work is retained in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), and dependency license texts are bundled in [THIRD_PARTY_LICENSES.html](THIRD_PARTY_LICENSES.html).
