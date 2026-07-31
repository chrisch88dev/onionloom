# Architecture

## Address path

A Tor v3 address encodes:

```text
public key, 32 bytes
checksum, SHA3-256(".onion checksum" || public key || version)[0..2]
version, one byte with value 3
```

The 35-byte payload is unpadded lowercase base32 followed by `.onion`.

The first 51 base32 characters depend only on the first 255 public-key bits. Onionloom compiles each requested prefix into byte values and masks, so the search loop does not allocate, base32-encode, or hash ordinary candidates. More than eight non-overlapping prefixes use first-character buckets before the masked comparison.

## Secret format

Tor stores Ed25519 secret keys in a 64-byte expanded representation:

```text
bytes 0..32: clamped scalar in little-endian form
bytes 32..64: signing nonce prefix
```

This differs from a standard 32-byte Ed25519 seed. Onionloom hashes a random 32-byte seed with SHA-512, clamps the first half, and preserves the second half while searching scalar offsets.

Tagged key files begin with a 32-byte NUL-padded header. Public files are 64 bytes total and secret files are 96 bytes total.

## Search ranges

Let `B` be the Ed25519 base point and `s` the random clamped scalar. Candidate `i` is:

```text
secret scalar: s + 8i
public point:  (s + 8i)B
```

Adding a multiple of eight preserves the low clamp bits. Workers own disjoint blocks. A configured paired batch of size `n` tests `n + 1` offsets including its center. Worker `w` starts at `w(n + 1)` and advances by `t(n + 1)` for worker count `t`.

Let `P` be a block center and let the shared table contain affine points `Q`. Onionloom uses the d-independent twisted Edwards formulas:

```text
y(P + Q) = (x1*y1 - x2*y2) / (x1*y2 - y1*x2)
y(P - Q) = (x1*y1 + x2*y2) / (x1*y2 + y1*x2)
```

The paired numerators share precomputed `x2*y2`. Simultaneous field division shares one inversion across the complete block. Sixteen independent prefix-product chains expose multiplication latency to the CPU and are combined before that one inversion. Reused heap buffers keep large batches off thread stacks. The offset table is immutable and shared by all workers, and automatic tuning measures paired batch sizes through 8,192 under the selected worker load.

The formulas are incomplete when the two odd-order inputs are equal or negatives. Onionloom checks every denominator. If an exceptional block is ever observed, it evaluates that exact range through the complete Dalek path instead of inverting zero or skipping work. Generated clamped scalars make this unreachable in the current `u64` offset domain, but the fallback preserves the invariant explicitly.

Only the compressed y coordinate is needed in the hot loop. Byte 31 bit 7 stores the x sign and affects prefix character 50. The y-only prefilter clears that one bit for 50 and 51 character patterns, then complete CPU reconstruction resolves the rare prehit without false negatives.

The CPU field engine uses safe Rust, four `u64` limbs, and exact `u128` products. Intermediate values remain weakly reduced below `2^255`; observable bytes, equality, parity, and zero checks use canonical field semantics. A specialized square uses ten wide products instead of the generic multiplication's sixteen. The engine has no custom assembly and unsafe code remains forbidden. `curve25519-dalek` supplies table setup and independent verification. The coordinate formulas are described in [Twisted Edwards Curves Revisited](https://eprint.iacr.org/2008/522), and simultaneous division follows the parallel-prefix method in [A Taxonomy of Parallel Prefix Networks](https://eprint.iacr.org/2008/199).

## GPU path

The default build uses safe `wgpu` host code and a WGSL compute shader. Adapter discovery uses Vulkan on Linux, DirectX 12 or Vulkan on Windows, and Metal on macOS. Hardware must support exact 64-bit integer shaders, at least 128 workgroup lanes, and the required workgroup storage. Each 128-lane workgroup owns an affine center, pairs 127 positive and negative offsets, and checks the center for 255 contiguous candidates per round. Its 256-leaf product tree batch-inverts 254 candidate denominators and the next center's projective `Z`. The last leaf is a multiplicative identity pad.

The GPU never receives the expanded secret. It receives compressed public starting points, a public stride point, and public prefix masks. A returned offset is untrusted. The host applies that offset to the expanded secret, recomputes the public key through `curve25519-dalek`, and requires the prefix to match before output.

GPU search is transactional. A dispatch advances center points only into scratch banks and writes y-coordinate prehits into a bounded ring. The host checks that every returned offset belongs to the exact dispatched group and round, rejects duplicates, reconstructs every prehit, and commits the scratch centers only after validation. A full ring causes the unchanged range to be replayed by round and then recursively split by workgroup. One leaf contains at most 255 candidates for a 256-entry ring, so replay terminates without dropping a candidate.

GPU and CPU searches use independent random base secrets, so hybrid work does not overlap. Automatic mode validates exact GPU arithmetic, tunes workgroup count, and requires a useful measured rate relative to the current CPU setup. Failure at any step falls back to CPU. Checkpoints currently describe only CPU ranges, so checkpoint searches automatically remain CPU-only and cannot be combined with forced GPU search.

## Found-key boundary

A masked prefix match is only a candidate. Before output, Onionloom:

1. Applies the recorded offset to a copy of the expanded base secret.
2. Checks that clamp bits remain valid.
3. Recomputes the public key with an independent base-point multiplication.
4. Requires the recomputed public key to match an exact requested prefix.
5. Recomputes and validates the onion address.
6. Requires the public key to match a requested filter.

## Output transaction

Output uses a new temporary service directory under the selected root. Files are created with new-file semantics and synced. Onionloom reads all three files back, parses exact headers and lengths, recomputes key relationships, checks the hostname checksum, and checks Unix secret permissions. It then renames the directory to its final onion hostname. Existing destinations are never replaced.

## Checkpoints

A checkpoint stores:

- schema version
- normalized patterns
- worker count and batch size
- expanded base secret
- next offset snapshot for every worker

Workers update offsets after each batch. A checkpoint can repeat part of an in-flight batch after interruption, but it cannot skip a completed batch reflected in the snapshot. Repeated work affects speed, not correctness.

Checkpoint state is sensitive and opt-in.
