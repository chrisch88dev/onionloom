# Benchmarking

Onionloom separates three measurements:

- Candidate engine measures point generation and batch compression.
- Filtered tuning adds the exact compiled patterns used by a search.
- Live progress divides completed candidates by search wall time.

ETA uses filtered tuning at startup and live progress after the search begins.

## Quick hardware selection

```console
cargo build --release
./target/release/onionloom benchmark --trial-ms 500
```

The tuner measures all supported batch sizes with one worker, then measures the best batch size at every available logical worker count.

## Comparison protocol

Use release builds made with equivalent CPU flags. Pin the machine to a consistent power source and performance policy. Stop unrelated heavy work. Record temperature or run enough alternating samples to expose thermal drift.

For each implementation:

1. Warm up for at least five seconds.
2. Run at least ten samples of at least ten seconds.
3. Alternate implementation order.
4. Use the same valid prefix set.
5. Include filter work in both results.
6. Report median, minimum, maximum, and median absolute deviation.
7. Run an end-to-end test that saves and validates at least one key.

Candidate counters must represent fully compressed public keys presented to the filter. Setup work and found-key validation should be reported separately when they are excluded from the rate.

## GPU acceptance gate

A GPU prototype does not qualify as supported merely because a kernel reports a high rate. It must beat the automatically tuned CPU path on end-to-end wall time for representative prefix lengths after including initialization, compilation, transfers, result recovery, key reconstruction, and validation.

Publish tested device and driver details. Keep the backend experimental if it wins only on selected hardware or only after excluding material setup costs.
