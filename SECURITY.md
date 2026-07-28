# Security policy

Onionloom handles long-lived Tor identity keys. Treat generated secret files and resume checkpoints as credentials.

## Supported versions

The project is pre-1.0. Security fixes are applied to the latest commit on the default branch. Tagged release support will be documented when stable releases begin.

## Report a vulnerability

Use GitHub private vulnerability reporting for `chrisch88dev/onionloom`. Do not open a public issue for a suspected vulnerability and do not include generated keys in any report.

Include:

- affected commit or version
- operating system and architecture
- minimal reproduction without real secret material
- expected and observed behavior
- security impact

Please allow a reasonable investigation window before public disclosure.

## Current security boundaries

Onionloom aims to protect key material from accidental disclosure and malformed output. It does not protect a compromised host, malicious kernel, hostile debugger, swap inspection, physical memory capture, or a parent directory with unsafe access controls.

The CPU search pattern and resulting public hostname are not secret. The expanded secret key, checkpoints, generated service directory, crash dumps containing process memory, and raw profiles may be secret.

## Platform behavior

On Unix:

- temporary and final service directories are created with mode `0700`
- generated files are created with mode `0600`
- the secret-key mode is checked during readback
- existing destination directories are never overwritten

On Windows, files inherit the parent directory access-control list. Until Onionloom configures and verifies a user-only ACL itself, create output inside a directory already restricted to the current account.

## Resume files

Resume state contains an expanded base secret. Anyone who obtains it and the offsets can reconstruct keys in that search range. Use checkpoints only on encrypted local storage. Onionloom removes the checkpoint after a successful search, but deletion is not guaranteed to erase blocks from solid-state media or copy-on-write filesystems.

## Cryptographic design

- randomness comes from the operating system through `getrandom`
- the expanded scalar uses the Ed25519 clamp required by Tor's format
- candidate offsets are multiples of eight
- secret buffers use `zeroize` on drop
- public keys are recomputed from found secret scalars before output
- onion checksums use SHA3-256 as specified by Tor
- secret data is never intentionally printed

This code has not yet received an external cryptographic audit. Do not interpret memory-safe Rust or passing tests as proof of cryptographic correctness.
