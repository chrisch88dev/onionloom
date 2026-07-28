//! Onionloom command-line interface.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand};

use onionloom::checkpoint::{Checkpoint, load as load_checkpoint, save as save_checkpoint};
use onionloom::engine::{BatchSize, start_search};
use onionloom::eta::{estimates, format_duration};
use onionloom::filter::{FilterSet, Pattern};
use onionloom::keys::ExpandedSecret;
use onionloom::output::{save_key, verify_directory};
use onionloom::tune::{Measurement, auto_tune, measure, measure_filtered};

#[derive(Debug, Parser)]
#[command(
    name = "onionloom",
    version,
    about = "Measured CPU-first Tor v3 onion vanity address generator",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Search for one or more Tor v3 address prefixes.
    Search(SearchArgs),
    /// Measure CPU candidate throughput without generating keys.
    Benchmark(BenchmarkArgs),
    /// Validate an existing Tor v3 key directory.
    Verify {
        /// Directory containing hostname and Tor Ed25519 key files.
        directory: PathBuf,
    },
}

#[derive(Debug, Args)]
struct SearchArgs {
    /// Lowercase base32 prefixes using a-z and 2-7.
    #[arg(required = true)]
    patterns: Vec<String>,

    /// Root directory for generated onion key directories.
    #[arg(short = 'd', long = "output", default_value = "onions")]
    output: PathBuf,

    /// Stop after this many keys.
    #[arg(short = 'n', long, default_value_t = 1)]
    count: usize,

    /// Worker threads. Omit to benchmark and select automatically.
    #[arg(short = 't', long)]
    workers: Option<usize>,

    /// Batch size: 64, 128, 256, 512, or 1024. Omit to benchmark.
    #[arg(long)]
    batch_size: Option<usize>,

    /// Milliseconds per automatic tuning trial.
    #[arg(long, default_value_t = 150, value_parser = clap::value_parser!(u64).range(25..))]
    tune_ms: u64,

    /// Save protected resume state here. The file contains secret material.
    #[arg(long)]
    checkpoint: Option<PathBuf>,

    /// Progress report interval in seconds.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..))]
    progress_seconds: u64,

    /// Do not print periodic progress.
    #[arg(short = 'q', long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct BenchmarkArgs {
    /// Worker threads. Omit to test every available count.
    #[arg(short = 't', long)]
    workers: Option<usize>,

    /// Batch size. Omit to test every supported size.
    #[arg(long)]
    batch_size: Option<usize>,

    /// Milliseconds per measured configuration.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(25..))]
    trial_ms: u64,
}

struct PreparedSearch {
    secret: ExpandedSecret,
    workers: usize,
    batch_size: BatchSize,
    resume_offsets: Option<Vec<u64>>,
    baseline: Measurement,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Search(arguments)) => search(arguments),
        Some(Command::Benchmark(arguments)) => benchmark(arguments),
        Some(Command::Verify { directory }) => {
            let verified = verify_directory(&directory)?;
            println!("{}", verified.hostname);
            Ok(())
        }
        None => bail!("choose a command; run onionloom --help for usage"),
    }
}

fn search(arguments: SearchArgs) -> Result<()> {
    ensure!(arguments.count > 0, "count must be greater than zero");
    if let Some(workers) = arguments.workers {
        ensure!(workers > 0, "worker count must be greater than zero");
    }
    let parsed_patterns = arguments
        .patterns
        .iter()
        .map(|pattern| Pattern::parse(pattern))
        .collect::<Result<Vec<_>>>()?;
    let filters = Arc::new(FilterSet::new(parsed_patterns)?);
    let normalized_patterns = filters
        .patterns()
        .iter()
        .map(|pattern| pattern.as_str().to_owned())
        .collect::<Vec<_>>();

    let PreparedSearch {
        secret,
        workers,
        batch_size,
        resume_offsets,
        baseline,
    } = prepare_search(&arguments, Arc::clone(&filters), &normalized_patterns)?;

    let probability = filters.probability();
    let estimate = estimates(probability, baseline.attempts_per_second);
    eprintln!(
        "engine: CPU batch-compress | workers: {workers} | batch: {} | filter: {}",
        batch_size.get(),
        filters.strategy()
    );
    eprintln!(
        "measured: {}/s over {:.3} s and {} candidates",
        format_rate(baseline.attempts_per_second),
        baseline.elapsed.as_secs_f64(),
        baseline.attempts
    );
    eprintln!(
        "next match: expected {}, median {}, 95% {}",
        format_duration(estimate.expected),
        format_duration(estimate.median),
        format_duration(estimate.p95)
    );
    if arguments.checkpoint.is_some() {
        eprintln!("checkpoint warning: the resume file contains secret key material");
    }

    let session = start_search(
        &secret,
        Arc::clone(&filters),
        workers,
        batch_size,
        resume_offsets,
    )?;
    let stop = Arc::clone(&session.stop);
    ctrlc::set_handler(move || {
        stop.store(true, Ordering::Relaxed);
    })
    .context("failed to install Ctrl-C handler")?;

    let started = Instant::now();
    let report_every = Duration::from_secs(arguments.progress_seconds);
    let mut last_report = Instant::now();
    let mut saved_count = 0_usize;
    while saved_count < arguments.count && !session.stop.load(Ordering::Relaxed) {
        match session.matches.recv_timeout(Duration::from_millis(100)) {
            Ok(found) => {
                let directory =
                    save_key(&arguments.output, &found.secret, &found.public, &filters)?;
                println!("{}", directory.display());
                saved_count += 1;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_report.elapsed() >= report_every {
            save_resume_state(
                arguments.checkpoint.as_deref(),
                &normalized_patterns,
                workers,
                batch_size,
                &secret,
                session.next_offsets(),
            )?;
            if !arguments.quiet {
                report_progress(session.attempts(), started.elapsed(), probability);
            }
            last_report = Instant::now();
        }
    }

    let interrupted = session.stop.load(Ordering::Relaxed) && saved_count < arguments.count;
    save_resume_state(
        arguments.checkpoint.as_deref(),
        &normalized_patterns,
        workers,
        batch_size,
        &secret,
        session.next_offsets(),
    )?;
    let attempts = session.attempts();
    session.stop_and_join()?;
    if !arguments.quiet {
        report_progress(attempts, started.elapsed(), probability);
    }

    if saved_count == arguments.count {
        if let Some(path) = arguments.checkpoint.as_deref()
            && path.exists()
        {
            std::fs::remove_file(path).with_context(|| {
                format!("failed to remove completed checkpoint {}", path.display())
            })?;
        }
        Ok(())
    } else if interrupted {
        bail!("search interrupted; protected checkpoint saved when configured")
    } else {
        bail!("all search workers stopped before enough keys were found")
    }
}

fn prepare_search(
    arguments: &SearchArgs,
    filters: Arc<FilterSet>,
    normalized_patterns: &[String],
) -> Result<PreparedSearch> {
    let saved = arguments
        .checkpoint
        .as_deref()
        .filter(|path| path.exists())
        .map(load_checkpoint)
        .transpose()?;
    if let Some(checkpoint) = saved.as_ref() {
        ensure!(
            checkpoint.patterns() == normalized_patterns,
            "checkpoint patterns differ from the requested patterns"
        );
        if let Some(requested) = arguments.workers {
            ensure!(
                requested == checkpoint.workers(),
                "worker count must match the checkpoint"
            );
        }
        if let Some(requested) = arguments.batch_size {
            ensure!(
                requested == checkpoint.batch_size(),
                "batch size must match the checkpoint"
            );
        }
        eprintln!(
            "resuming {} workers, batch {}, from {}",
            checkpoint.workers(),
            checkpoint.batch_size(),
            arguments
                .checkpoint
                .as_deref()
                .expect("checkpoint path exists")
                .display()
        );
        let batch_size = BatchSize::parse(checkpoint.batch_size())?;
        let baseline = measure_filtered(
            checkpoint.workers(),
            batch_size,
            Duration::from_millis(arguments.tune_ms),
            filters,
        )?;
        return Ok(PreparedSearch {
            secret: checkpoint.base_secret()?,
            workers: checkpoint.workers(),
            batch_size,
            resume_offsets: Some(checkpoint.next_offsets().to_vec()),
            baseline,
        });
    }

    let requested_batch = arguments.batch_size.map(BatchSize::parse).transpose()?;
    let (workers, batch_size, baseline) = select_configuration(
        arguments.workers,
        requested_batch,
        arguments.tune_ms,
        filters,
    )?;
    Ok(PreparedSearch {
        secret: ExpandedSecret::generate()?,
        workers,
        batch_size,
        resume_offsets: None,
        baseline,
    })
}

fn select_configuration(
    workers: Option<usize>,
    batch_size: Option<BatchSize>,
    trial_ms: u64,
    filters: Arc<FilterSet>,
) -> Result<(usize, BatchSize, Measurement)> {
    let duration = Duration::from_millis(trial_ms);
    match (workers, batch_size) {
        (Some(workers), Some(batch)) => Ok((
            workers,
            batch,
            measure_filtered(workers, batch, duration, filters)?,
        )),
        (Some(workers), None) => {
            let mut measurements = Vec::new();
            for batch in BatchSize::ALL {
                measurements.push(measure_filtered(
                    workers,
                    batch,
                    duration,
                    Arc::clone(&filters),
                )?);
            }
            let best = *measurements
                .iter()
                .max_by(|left, right| {
                    left.attempts_per_second
                        .total_cmp(&right.attempts_per_second)
                })
                .expect("batch options are non-empty");
            Ok((workers, best.batch_size, best))
        }
        (None, Some(batch)) => {
            let logical = std::thread::available_parallelism()?.get();
            let mut measurements = Vec::new();
            for workers in 1..=logical {
                measurements.push(measure_filtered(
                    workers,
                    batch,
                    duration,
                    Arc::clone(&filters),
                )?);
            }
            let best = *measurements
                .iter()
                .max_by(|left, right| {
                    left.attempts_per_second
                        .total_cmp(&right.attempts_per_second)
                })
                .expect("worker options are non-empty");
            Ok((best.workers, batch, best))
        }
        (None, None) => {
            eprintln!("auto-tuning CPU; each trial runs for {trial_ms} ms");
            let tuning = auto_tune(duration, Some(filters))?;
            Ok((
                tuning.selected.workers,
                tuning.selected.batch_size,
                tuning.selected,
            ))
        }
    }
}

fn save_resume_state(
    path: Option<&Path>,
    patterns: &[String],
    workers: usize,
    batch_size: BatchSize,
    secret: &ExpandedSecret,
    offsets: Vec<u64>,
) -> Result<()> {
    if let Some(path) = path {
        let checkpoint = Checkpoint::new(
            patterns.to_vec(),
            workers,
            batch_size.get(),
            secret,
            offsets,
        );
        save_checkpoint(path, &checkpoint)?;
    }
    Ok(())
}

fn report_progress(attempts: u64, elapsed: Duration, probability: f64) {
    let rate = attempts as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let estimate = estimates(probability, rate);
    eprintln!(
        "progress: {attempts} candidates | {}/s | next median {} | 95% {}",
        format_rate(rate),
        format_duration(estimate.median),
        format_duration(estimate.p95)
    );
}

fn benchmark(arguments: BenchmarkArgs) -> Result<()> {
    let duration = Duration::from_millis(arguments.trial_ms);
    let requested_batch = arguments.batch_size.map(BatchSize::parse).transpose()?;
    match (arguments.workers, requested_batch) {
        (Some(workers), Some(batch)) => print_measurement(measure(workers, batch, duration)?),
        (Some(workers), None) => {
            ensure!(workers > 0, "worker count must be greater than zero");
            for batch in BatchSize::ALL {
                print_measurement(measure(workers, batch, duration)?);
            }
        }
        (None, Some(batch)) => {
            for workers in 1..=std::thread::available_parallelism()?.get() {
                print_measurement(measure(workers, batch, duration)?);
            }
        }
        (None, None) => {
            let tuning = auto_tune(duration, None)?;
            println!("batch-size trials:");
            for measurement in tuning.batch_measurements {
                print_measurement(measurement);
            }
            println!("worker-count trials:");
            for measurement in tuning.worker_measurements {
                print_measurement(measurement);
            }
            println!(
                "selected: {} workers, batch {}, {}/s",
                tuning.selected.workers,
                tuning.selected.batch_size.get(),
                format_rate(tuning.selected.attempts_per_second)
            );
        }
    }
    Ok(())
}

fn print_measurement(measurement: Measurement) {
    println!(
        "workers={} batch={} candidates={} elapsed={:.3}s rate={}/s",
        measurement.workers,
        measurement.batch_size.get(),
        measurement.attempts,
        measurement.elapsed.as_secs_f64(),
        format_rate(measurement.attempts_per_second)
    );
}

fn format_rate(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!("{:.2}G", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.2}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.2}k", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}
