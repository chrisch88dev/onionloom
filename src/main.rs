//! Onionloom command-line interface.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

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
    about = "Create custom Tor v3 onion addresses",
    long_about = "Create custom Tor v3 onion addresses with measured estimates and safe defaults.\n\nOnionloom checks this computer before searching, chooses efficient CPU settings, and explains how long a pattern may take. It never needs a network connection.",
    after_help = "QUICK START:\n  Check this computer:       onionloom evaluate\n  Estimate a name:           onionloom guess myname\n  Start searching:           onionloom search myname\n  Use maximum CPU capacity:  onionloom search myname --power full\n  Explain a command:         onionloom help search"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate an onion address beginning with your chosen text.
    Search(SearchArgs),
    /// Check this computer and explain expected search times.
    #[command(visible_alias = "eval")]
    Evaluate(EvaluateArgs),
    /// Estimate a pattern on this computer without generating a key.
    #[command(visible_alias = "estimate")]
    Guess(GuessArgs),
    /// Run detailed CPU measurements for performance work.
    Benchmark(BenchmarkArgs),
    /// Validate an existing Tor v3 key directory.
    Verify {
        /// Directory containing hostname and Tor Ed25519 key files.
        directory: PathBuf,
    },
}

#[derive(Debug, Args)]
struct SearchArgs {
    /// Desired address beginnings, such as loom or private2.
    ///
    /// Use lowercase letters a-z and digits 2-7. You may provide several
    /// patterns and Onionloom will stop when any one of them matches.
    #[arg(required = true)]
    patterns: Vec<String>,

    /// Root directory for generated onion key directories.
    #[arg(short = 'd', long = "output", default_value = "onions")]
    output: PathBuf,

    /// Stop after this many keys.
    #[arg(short = 'n', long, default_value_t = 1)]
    count: usize,

    /// CPU use: low, medium, or full. The default is medium.
    #[arg(long, value_enum, conflicts_with = "workers")]
    power: Option<PowerMode>,

    /// Exact number of parallel search tasks. Usually use --power instead.
    #[arg(short = 't', long, help_heading = "Advanced")]
    workers: Option<usize>,

    /// Addresses processed together: 64, 128, 256, 512, or 1024.
    #[arg(long, help_heading = "Advanced")]
    batch_size: Option<usize>,

    /// Milliseconds spent measuring each automatic setup.
    #[arg(long, default_value_t = 150, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    tune_ms: u64,

    /// Save protected resume state here. The file contains secret material.
    #[arg(long, help_heading = "Resume and output")]
    checkpoint: Option<PathBuf>,

    /// Progress report interval in seconds.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..), help_heading = "Progress")]
    progress_seconds: u64,

    /// Do not print periodic progress.
    #[arg(short = 'q', long, help_heading = "Progress")]
    quiet: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PowerMode {
    /// Leave most CPU capacity available for other work.
    Low,
    /// Balance search speed with a responsive computer.
    Medium,
    /// Allow Onionloom to use every available CPU processing slot.
    Full,
}

impl PowerMode {
    fn worker_limit(self, available: usize) -> usize {
        match self {
            Self::Low => available.div_ceil(4),
            Self::Medium => available.div_ceil(2),
            Self::Full => available,
        }
        .max(1)
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::Full => "full",
        }
    }

    const fn explanation(self) -> &'static str {
        match self {
            Self::Low => "leaves most CPU capacity for other work",
            Self::Medium => "balances search speed and responsiveness",
            Self::Full => "allows every available CPU processing slot",
        }
    }
}

#[derive(Debug, Args)]
struct EvaluateArgs {
    /// CPU use: low, medium, or full. The default is medium.
    #[arg(long, value_enum)]
    power: Option<PowerMode>,

    /// Milliseconds spent measuring each setup.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    trial_ms: u64,
}

#[derive(Debug, Args)]
struct GuessArgs {
    /// Address beginnings to estimate, such as loom or private2.
    ///
    /// Use lowercase letters a-z and digits 2-7. With several patterns,
    /// the estimate is for finding any one of them.
    #[arg(required = true)]
    patterns: Vec<String>,

    /// CPU use: low, medium, or full. The default is medium.
    #[arg(long, value_enum)]
    power: Option<PowerMode>,

    /// Milliseconds spent measuring each setup.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    trial_ms: u64,
}

#[derive(Debug, Args)]
struct BenchmarkArgs {
    /// CPU range to measure: low, medium, or full. The default is full.
    #[arg(long, value_enum, conflicts_with = "workers")]
    power: Option<PowerMode>,

    /// Exact parallel search task count. Omit to test the power range.
    #[arg(short = 't', long, help_heading = "Advanced")]
    workers: Option<usize>,

    /// Batch size. Omit to test every supported size.
    #[arg(long, help_heading = "Advanced")]
    batch_size: Option<usize>,

    /// Milliseconds per measured configuration.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
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
        Some(Command::Evaluate(arguments)) => evaluate(arguments),
        Some(Command::Guess(arguments)) => guess(arguments),
        Some(Command::Benchmark(arguments)) => benchmark(arguments),
        Some(Command::Verify { directory }) => {
            let verified = verify_directory(&directory)?;
            println!("Valid Tor v3 key directory");
            println!("Address: {}", verified.hostname);
            Ok(())
        }
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

fn search(arguments: SearchArgs) -> Result<()> {
    ensure!(arguments.count > 0, "count must be greater than zero");
    if let Some(workers) = arguments.workers {
        ensure!(workers > 0, "worker count must be greater than zero");
    }
    let filters = parse_filters(&arguments.patterns)?;
    let normalized_patterns = filters
        .patterns()
        .iter()
        .map(|pattern| pattern.as_str().to_owned())
        .collect::<Vec<_>>();
    let resumed = arguments.checkpoint.as_deref().is_some_and(Path::exists);

    let PreparedSearch {
        secret,
        workers,
        batch_size,
        resume_offsets,
        baseline,
    } = prepare_search(&arguments, Arc::clone(&filters), &normalized_patterns)?;

    let probability = filters.probability();
    let estimate = estimates(probability, baseline.attempts_per_second);
    let available = std::thread::available_parallelism()?.get();
    if resumed {
        eprintln!("CPU setting: restored from the resume file");
    } else if let Some(requested_workers) = arguments.workers {
        eprintln!("CPU setting: custom ({requested_workers} parallel search tasks)");
    } else {
        let power = arguments.power.unwrap_or(PowerMode::Medium);
        print_power_mode(power, available);
    }
    eprintln!("Chosen setup: {workers} parallel search tasks");
    if arguments.batch_size.is_some() {
        eprintln!(
            "Advanced batch setting: {} possible addresses processed together",
            batch_size.get()
        );
    }
    eprintln!(
        "Measured speed: {}",
        format_rate_plain(baseline.attempts_per_second)
    );
    eprintln!(
        "Difficulty: about 1 match per {} possible addresses tested",
        format_count(1.0 / probability)
    );
    print_time_estimates(estimate);
    if arguments.checkpoint.is_some() {
        eprintln!("Security warning: the resume file contains secret key material.");
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
                println!("Saved verified key: {}", directory.display());
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
        bail!("search interrupted; a protected resume file was saved if --checkpoint was set")
    } else {
        bail!("all search workers stopped before enough keys were found")
    }
}

fn parse_filters(patterns: &[String]) -> Result<Arc<FilterSet>> {
    let parsed = patterns
        .iter()
        .map(|pattern| Pattern::parse(pattern))
        .collect::<Result<Vec<_>>>()?;
    Ok(Arc::new(FilterSet::new(parsed)?))
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
            "Resuming saved work with {} parallel search tasks from {}",
            checkpoint.workers(),
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
        arguments.power.unwrap_or(PowerMode::Medium),
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
    power: PowerMode,
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
            let worker_limit = power.worker_limit(logical);
            let mut measurements = Vec::new();
            for workers in 1..=worker_limit {
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
            let available = std::thread::available_parallelism()?.get();
            let worker_limit = power.worker_limit(available);
            eprintln!(
                "Checking this computer to choose the fastest setup within {} power mode...",
                power.label()
            );
            let tuning = auto_tune(duration, Some(filters), worker_limit)?;
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
        "Tested {} possible addresses | {} | 50% ETA {} | 95% ETA {}",
        format_count(attempts as f64),
        format_rate_plain(rate),
        format_duration(estimate.median),
        format_duration(estimate.p95)
    );
}

fn evaluate(arguments: EvaluateArgs) -> Result<()> {
    let power = arguments.power.unwrap_or(PowerMode::Medium);
    let available = std::thread::available_parallelism()?.get();
    let worker_limit = power.worker_limit(available);
    println!("Checking this computer. No onion key will be generated...");
    let tuning = auto_tune(
        Duration::from_millis(arguments.trial_ms),
        None,
        worker_limit,
    )?;

    println!();
    println!("Hardware check complete");
    print_power_mode_stdout(power, available);
    println!(
        "Recommended setup: {} parallel search tasks",
        tuning.selected.workers
    );
    println!(
        "Measured speed: {}",
        format_rate_plain(tuning.selected.attempts_per_second)
    );
    println!();
    println!("Estimated time for one exact address beginning");
    println!("Characters  Average time  50% chance by  95% chance by");
    for length in 3..=10 {
        let probability = 2_f64.powi(-5 * length);
        let estimate = estimates(probability, tuning.selected.attempts_per_second);
        println!(
            "{length:>10}  {:>12}  {:>13}  {:>13}",
            format_duration(estimate.expected),
            format_duration(estimate.median),
            format_duration(estimate.p95)
        );
    }
    println!();
    println!("Average time is the long-run mean, not a deadline.");
    println!("50% ETA means half of searches should finish by that time.");
    println!("95% ETA means 95 out of 100 searches should finish by that time.");
    println!("Use `onionloom guess NAME` for an estimate of a specific pattern.");
    Ok(())
}

fn guess(arguments: GuessArgs) -> Result<()> {
    let filters = parse_filters(&arguments.patterns)?;
    let power = arguments.power.unwrap_or(PowerMode::Medium);
    let available = std::thread::available_parallelism()?.get();
    let worker_limit = power.worker_limit(available);
    println!("Checking this computer with the requested pattern. No key will be generated...");
    let tuning = auto_tune(
        Duration::from_millis(arguments.trial_ms),
        Some(Arc::clone(&filters)),
        worker_limit,
    )?;
    let probability = filters.probability();
    let estimate = estimates(probability, tuning.selected.attempts_per_second);

    println!();
    println!(
        "Pattern{}: {}",
        if filters.patterns().len() == 1 {
            ""
        } else {
            "s"
        },
        filters
            .patterns()
            .iter()
            .map(Pattern::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    );
    print_power_mode_stdout(power, available);
    println!(
        "Recommended setup: {} parallel search tasks",
        tuning.selected.workers
    );
    println!(
        "Measured speed: {}",
        format_rate_plain(tuning.selected.attempts_per_second)
    );
    println!(
        "Difficulty: about 1 match per {} possible addresses tested",
        format_count(1.0 / probability)
    );
    print_time_estimates_stdout(estimate);
    println!("This is a probability estimate, not a completion deadline.");
    Ok(())
}

fn benchmark(arguments: BenchmarkArgs) -> Result<()> {
    let duration = Duration::from_millis(arguments.trial_ms);
    let requested_batch = arguments.batch_size.map(BatchSize::parse).transpose()?;
    println!(
        "Each rate is the number of possible addresses generated and checked per wall-clock second."
    );
    match (arguments.workers, requested_batch) {
        (Some(workers), Some(batch)) => print_measurement(measure(workers, batch, duration)?),
        (Some(workers), None) => {
            ensure!(workers > 0, "worker count must be greater than zero");
            for batch in BatchSize::ALL {
                print_measurement(measure(workers, batch, duration)?);
            }
        }
        (None, Some(batch)) => {
            let available = std::thread::available_parallelism()?.get();
            let power = arguments.power.unwrap_or(PowerMode::Full);
            for workers in 1..=power.worker_limit(available) {
                print_measurement(measure(workers, batch, duration)?);
            }
        }
        (None, None) => {
            let available = std::thread::available_parallelism()?.get();
            let power = arguments.power.unwrap_or(PowerMode::Full);
            let tuning = auto_tune(duration, None, power.worker_limit(available))?;
            println!("batch-size trials:");
            for measurement in tuning.batch_measurements {
                print_measurement(measurement);
            }
            println!("worker-count trials:");
            for measurement in tuning.worker_measurements {
                print_measurement(measurement);
            }
            println!(
                "selected: {} workers, batch {}, {}",
                tuning.selected.workers,
                tuning.selected.batch_size.get(),
                format_rate_plain(tuning.selected.attempts_per_second)
            );
        }
    }
    Ok(())
}

fn print_measurement(measurement: Measurement) {
    println!(
        "workers={} batch={} addresses_tested={} elapsed={:.3}s rate={}",
        measurement.workers,
        measurement.batch_size.get(),
        measurement.attempts,
        measurement.elapsed.as_secs_f64(),
        format_rate_plain(measurement.attempts_per_second)
    );
}

fn format_rate_plain(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!(
            "{:.2} billion possible addresses/second",
            value / 1_000_000_000.0
        )
    } else if value >= 1_000_000.0 {
        format!(
            "{:.2} million possible addresses/second",
            value / 1_000_000.0
        )
    } else if value >= 1_000.0 {
        format!("{:.2} thousand possible addresses/second", value / 1_000.0)
    } else {
        format!("{value:.0} possible addresses/second")
    }
}

fn print_power_mode(power: PowerMode, available: usize) {
    let limit = power.worker_limit(available);
    eprintln!(
        "Power mode: {} (runs up to {limit} parallel search tasks; {available} CPU processing slots are available; {})",
        power.label(),
        power.explanation()
    );
}

fn print_power_mode_stdout(power: PowerMode, available: usize) {
    let limit = power.worker_limit(available);
    println!(
        "Power mode: {} (up to {limit} parallel search tasks; {available} CPU processing slots available; {})",
        power.label(),
        power.explanation()
    );
}

fn print_time_estimates(estimate: onionloom::eta::Estimates) {
    eprintln!("Time estimate for the next match:");
    eprintln!("  Average time: {}", format_duration(estimate.expected));
    eprintln!("  50% chance by: {}", format_duration(estimate.median));
    eprintln!("  95% chance by: {}", format_duration(estimate.p95));
    eprintln!("These are probability estimates, not deadlines.");
}

fn print_time_estimates_stdout(estimate: onionloom::eta::Estimates) {
    println!("Time estimate:");
    println!("  Average time: {}", format_duration(estimate.expected));
    println!("  50% chance by: {}", format_duration(estimate.median));
    println!("  95% chance by: {}", format_duration(estimate.p95));
}

fn format_count(value: f64) -> String {
    if !value.is_finite() || value > u64::MAX as f64 {
        return format!("{value:.3e}");
    }
    let digits = format!("{:.0}", value.max(0.0));
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    let first_group = digits.len() % 3;
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && index % 3 == first_group {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, PowerMode, format_count};

    #[test]
    fn power_modes_bound_available_threads() {
        assert_eq!(PowerMode::Low.worker_limit(1), 1);
        assert_eq!(PowerMode::Low.worker_limit(12), 3);
        assert_eq!(PowerMode::Medium.worker_limit(12), 6);
        assert_eq!(PowerMode::Full.worker_limit(12), 12);
        assert_eq!(PowerMode::Medium.worker_limit(3), 2);
    }

    #[test]
    fn guess_alias_and_plain_power_flag_parse() {
        let cli = Cli::try_parse_from(["onionloom", "estimate", "loom", "--power", "full"])
            .expect("valid command");
        let Some(Command::Guess(arguments)) = cli.command else {
            panic!("guess command expected");
        };
        assert_eq!(arguments.patterns, ["loom"]);
        assert_eq!(arguments.power, Some(PowerMode::Full));
    }

    #[test]
    fn power_and_exact_workers_conflict() {
        let parsed = Cli::try_parse_from([
            "onionloom",
            "search",
            "loom",
            "--power",
            "full",
            "--workers",
            "4",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    fn counts_are_grouped_for_readability() {
        assert_eq!(format_count(1_048_576.0), "1,048,576");
        assert_eq!(format_count(32.0), "32");
    }
}
