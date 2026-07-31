# Benchmarking

Onionloom separates three measurements:

- Possible-address engine measures paired-coordinate generation and simultaneous field division.
- Filtered tuning adds the exact compiled patterns used by a search.
- Live progress divides completed possible addresses by search wall time.

ETA uses filtered tuning at startup and live progress after the search begins.

For an automatically selected GPU, the initial ETA combines adjacent CPU and GPU measurements. Live progress is the authoritative concurrent rate because shared power, cooling, memory, and scheduling can prevent rates from adding perfectly.

## Quick hardware selection

```console
./build.sh
./onionloom benchmark --power full --trial-ms 10000
./onionloom gpu
```

The tuner measures every supported batch size at the maximum worker count allowed by the selected power mode. It then measures the winning batch at every allowed worker count. This captures shared-cache, clock, and memory effects that a one-worker batch test cannot see. It confirms the selected setup with three timed samples and uses the median sample for estimates.

## Comparison protocol

Use release builds made with equivalent CPU flags. Pin the machine to a consistent power source and performance policy. Stop unrelated heavy work. Record temperature or run enough alternating samples to expose thermal drift. A `target-cpu=native` binary is valid only for comparisons on the machine that built it.

For each implementation:

1. Warm up for at least five seconds.
2. Run at least ten samples of at least ten seconds.
3. Alternate implementation order.
4. Use the same valid prefix set.
5. Include filter work in both results.
6. Report median, minimum, maximum, and median absolute deviation.
7. Run an end-to-end test that saves and validates at least one key.

Possible-address counters must represent complete public y coordinates presented to the prefilter. For 50 and 51 character prefixes, record that x-sign verification occurs after a prehit. Setup work and found-key validation should be reported separately when they are excluded from the rate.

Every published result must include the exact Git commit, build profile, compiler flags, CPU model, operating system and kernel, logical CPU count, worker count, batch size, pattern set, trial duration, warmup, sample count, and reported median, minimum, maximum, and variation. Do not label a result as faster, best, or market-leading until an equivalent same-machine comparison supports it.

## GPU measurement

The automatic GPU policy does not rely on a device name or a theoretical operations count. It checks required features, validates exact arithmetic against the CPU, measures complete candidate work, and requires a useful rate relative to the tuned CPU setup.

Published GPU comparisons must include initialization, shader compilation, transfers, result recovery, key reconstruction, and validation. A kernel-only rate is not an end-to-end search result.

`onionloom gpu` measures complete GPU candidate generation, including command submission and result readback after initialization. Record its selected adapter, backend, workgroup count, and persistent lane count. For a hybrid claim, compare a normal automatic search with an immediately adjacent `search --gpu off` run using the same power policy and prefix workload.

## Publishing a comparison

Do not publish personal development machine snapshots in project-facing documentation. Store raw logs outside the repository, remove generated keys, and publish only a reproducible report tied to a release or commit.

Every speed percentage or ranking is still a benchmark claim. The report must include:

- exact Onionloom and competitor source commits
- exact build profile, compiler versions, and flags
- CPU, GPU, driver, operating system, and kernel
- worker count, batch size, pattern set, warmup, duration, and sample count
- alternating sample order, median, minimum, maximum, and median absolute deviation
- whether setup, tuning, shutdown, and key verification are inside each measured interval
- an end-to-end saved key that passes `onionloom verify`

Without that report, describe the architecture and invite users to run the protocol. Do not claim that Onionloom is universally fastest based on one private machine.
