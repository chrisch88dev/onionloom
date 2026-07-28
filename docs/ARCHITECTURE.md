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

Adding a multiple of eight preserves the low clamp bits. Workers own disjoint batches. For worker `w`, batch size `n`, and worker count `t`, its initial offset is `wn` and its next batch begins at `wn + tn`. Each worker performs one scalar multiplication to enter its range, then advances points with additions.

The batch compressor converts many extended Edwards points to compressed public keys with one batch inversion. Supported compile-time batch sizes avoid heap allocation in the hot loop.

## Found-key boundary

A masked prefix match is only a candidate. Before output, Onionloom:

1. Applies the recorded offset to a copy of the expanded base secret.
2. Checks that clamp bits remain valid.
3. Recomputes the public key with an independent base-point multiplication.
4. Requires byte-for-byte equality with the batch result.
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
