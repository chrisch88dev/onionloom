//! Vanity search command.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, ValueEnum};

use onionloom::crypto::keys::ExpandedSecret;
use onionloom::gpu::{GpuSearchSession, start_search as start_gpu_search};
use onionloom::performance::eta::{estimates, format_duration};
use onionloom::performance::tune::{
    Measurement, auto_tune, measure_filtered, measure_filtered_median,
};
use onionloom::search::cpu::{BatchSize, start_search};
use onionloom::search::filter::FilterSet;
use onionloom::storage::checkpoint::{
    Checkpoint, load as load_checkpoint, save as save_checkpoint,
};
use onionloom::storage::output::save_key;

use crate::cpu::PowerMode;
use crate::helpers::{
    automatic_gpu_is_worth_starting, automatic_gpu_rate_is_useful, format_count, format_rate,
    parse_filters, print_power_mode_stderr, print_time_estimates_stderr,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum GpuMode {
    /// Measure compatible hardware and use it only when it should help.
    #[default]
    Auto,
    /// Use the tuned CPU engine only.
    Off,
    /// Require a compatible GPU even when automatic policy would skip it.
    Force,
}

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    /// Desired address beginnings, such as loom or private2.
    ///
    /// Use lowercase letters a-z and digits 2-7. You may provide several
    /// patterns. Onionloom saves matches from any pattern.
    #[arg(required = true)]
    pub(crate) patterns: Vec<String>,

    /// Root directory for generated onion key directories.
    #[arg(short = 'd', long = "output", default_value = "onions")]
    output: PathBuf,

    /// Stop after this many saved keys. The default is one key.
    #[arg(short = 'n', long, conflicts_with = "continuous")]
    pub(crate) count: Option<usize>,

    /// Keep searching and saving matching keys until Ctrl-C.
    ///
    /// Without this control, Onionloom stops after one safely saved key.
    #[arg(long, visible_alias = "keep-going")]
    pub(crate) continuous: bool,

    /// CPU use: low, medium, or full. The default is full.
    #[arg(long, value_enum, conflicts_with = "workers")]
    pub(crate) power: Option<PowerMode>,

    /// GPU use: auto, off, or force. The default is auto.
    ///
    /// Auto validates and measures compatible hardware before enabling it.
    /// Every GPU result is independently verified on the CPU.
    #[arg(long, value_enum, default_value_t = GpuMode::Auto)]
    pub(crate) gpu: GpuMode,

    /// Exact number of parallel search tasks. Usually use --power instead.
    #[arg(short = 't', long, help_heading = "Advanced")]
    workers: Option<usize>,

    /// Paired work size: 64, 128, 256, 512, 1024, 2048, 4096, or 8192.
    #[arg(long, help_heading = "Advanced")]
    batch_size: Option<usize>,

    /// Milliseconds spent measuring each automatic setup.
    #[arg(long, default_value_t = 150, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    tune_ms: u64,

    /// Save protected resume state here. The file contains secret material.
    #[arg(long, help_heading = "Resume and output")]
    checkpoint: Option<PathBuf>,

    /// Progress report interval in seconds. Reports appear every 2 seconds by default.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u64).range(1..), hide = true)]
    progress_seconds: u64,

    /// Do not print progress reports, which appear every 2 seconds by default.
    #[arg(short = 'q', long, help_heading = "Progress")]
    quiet: bool,
}

struct PreparedSearch {
    secret: ExpandedSecret,
    workers: usize,
    batch_size: BatchSize,
    resume_offsets: Option<Vec<u64>>,
    baseline: Measurement,
}

pub(crate) fn run(arguments: SearchArgs) -> Result<()> {
    let requested_count = arguments.count.unwrap_or(1);
    ensure!(requested_count > 0, "count must be greater than zero");
    if let Some(workers) = arguments.workers {
        ensure!(workers > 0, "worker count must be greater than zero");
    }
    let filters = parse_filters(&arguments.patterns)?;
    let probability = filters.probability();
    ensure!(
        !(arguments.gpu == GpuMode::Force && arguments.checkpoint.is_some()),
        "`--gpu force` cannot be combined with a resume checkpoint; use `--gpu off` or omit the GPU setting"
    );
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

    let available = std::thread::available_parallelism()?.get();
    if resumed {
        eprintln!("CPU setting: restored from the resume file");
    } else if let Some(requested_workers) = arguments.workers {
        eprintln!("CPU setting: custom ({requested_workers} parallel search tasks)");
    } else {
        print_power_mode_stderr(arguments.power.unwrap_or(PowerMode::DEFAULT), available);
    }
    eprintln!("Chosen CPU setup: {workers} parallel search tasks");
    eprintln!(
        "Paired candidate batch size: {} plus one center address",
        batch_size.get()
    );
    eprintln!(
        "Measured CPU speed on this system: {}",
        format_rate(baseline.attempts_per_second)
    );
    let started = Instant::now();
    let gpu_session = prepare_gpu_search(
        &arguments,
        Arc::clone(&filters),
        probability,
        baseline.attempts_per_second,
    )?;
    let selected_rate = baseline.attempts_per_second
        + gpu_session
            .as_ref()
            .map_or(0.0, GpuSearchSession::measured_rate);
    if gpu_session.is_some() {
        eprintln!(
            "Initial combined CPU and GPU rate estimate: {}",
            format_rate(selected_rate)
        );
    }
    let estimate = estimates(probability, selected_rate);
    eprintln!(
        "Difficulty: about 1 match per {} possible addresses tested",
        format_count(1.0 / probability)
    );
    print_time_estimates_stderr(estimate);
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
    let gpu_stop = gpu_session.as_ref().map(GpuSearchSession::stop_flag);
    let interrupted_by_user = Arc::new(AtomicBool::new(false));
    let handler_interrupted_by_user = Arc::clone(&interrupted_by_user);
    ctrlc::set_handler(move || {
        handler_interrupted_by_user.store(true, Ordering::Relaxed);
        stop.store(true, Ordering::Relaxed);
        if let Some(gpu_stop) = gpu_stop.as_ref() {
            gpu_stop.store(true, Ordering::Relaxed);
        }
    })
    .context("failed to install Ctrl-C handler")?;

    let report_every = Duration::from_secs(arguments.progress_seconds);
    let mut last_report = Instant::now();
    let mut saved_count = 0_usize;
    while (arguments.continuous || saved_count < requested_count)
        && !session.stop.load(Ordering::Relaxed)
    {
        match session.matches.recv_timeout(Duration::from_millis(50)) {
            Ok(found) => {
                let directory =
                    save_key(&arguments.output, &found.secret, &found.public, &filters)?;
                println!("Saved verified key: {}", directory.display());
                saved_count += 1;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if gpu_session.is_none() {
                    break;
                }
            }
        }
        if let Some(found) = gpu_session
            .as_ref()
            .map(GpuSearchSession::try_match)
            .transpose()?
            .flatten()
        {
            let directory = save_key(&arguments.output, &found.secret, &found.public, &filters)?;
            println!("Saved verified GPU key: {}", directory.display());
            saved_count += 1;
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
                report_progress(
                    combined_attempts(&session, gpu_session.as_ref()),
                    started.elapsed(),
                    probability,
                );
            }
            last_report = Instant::now();
        }
    }

    let completed = !arguments.continuous && saved_count == requested_count;
    save_resume_state(
        arguments.checkpoint.as_deref(),
        &normalized_patterns,
        workers,
        batch_size,
        &secret,
        session.next_offsets(),
    )?;
    let attempts = combined_attempts(&session, gpu_session.as_ref());
    let mut dropped_matches = session.stop_and_join_with_dropped()?;
    if let Some(gpu_session) = gpu_session {
        dropped_matches = dropped_matches.saturating_add(gpu_session.stop_and_join()?);
    }
    if !arguments.quiet {
        report_progress(attempts, started.elapsed(), probability);
    }
    if dropped_matches > 0 {
        eprintln!(
            "Skipped {dropped_matches} matching candidate(s) because saving keys could not keep up. Use a more selective pattern to save every match."
        );
    }

    if completed {
        remove_completed_checkpoint(arguments.checkpoint.as_deref())?;
        Ok(())
    } else if interrupted_by_user.load(Ordering::Relaxed) {
        if arguments.continuous {
            eprintln!("Continuous search stopped after saving {saved_count} key(s).");
            Ok(())
        } else {
            bail!("search interrupted; a protected resume file was saved if --checkpoint was set")
        }
    } else if arguments.continuous {
        bail!("all search workers stopped during continuous search")
    } else {
        bail!("all search workers stopped before enough keys were found")
    }
}

fn prepare_gpu_search(
    arguments: &SearchArgs,
    filters: Arc<FilterSet>,
    probability: f64,
    cpu_rate: f64,
) -> Result<Option<GpuSearchSession>> {
    if arguments.gpu == GpuMode::Off {
        eprintln!("GPU setting: off");
        return Ok(None);
    }
    if arguments.checkpoint.is_some() {
        eprintln!("GPU setting: automatic CPU-only mode while a resume checkpoint is active");
        return Ok(None);
    }
    if !onionloom::gpu::compiled() {
        ensure!(
            arguments.gpu != GpuMode::Force,
            "this binary has no GPU engine; install the default build or run `./build.sh`"
        );
        return Ok(None);
    }
    let difficulty = 1.0 / probability;
    if arguments.gpu == GpuMode::Auto && !automatic_gpu_is_worth_starting(difficulty, cpu_rate) {
        eprintln!(
            "GPU setting: automatic CPU-only mode for this search estimate; GPU startup and result recovery are unlikely to pay back"
        );
        return Ok(None);
    }

    eprintln!("GPU setting: validating and measuring compatible hardware...");
    let session = match start_gpu_search(filters) {
        Ok(session) => session,
        Err(error) if arguments.gpu == GpuMode::Auto => {
            eprintln!("GPU setting: unavailable ({error}); continuing with the tuned CPU engine");
            return Ok(None);
        }
        Err(error) => return Err(error).context("required GPU initialization failed"),
    };
    let measured_rate = session.measured_rate();
    if arguments.gpu == GpuMode::Auto && !automatic_gpu_rate_is_useful(cpu_rate, measured_rate) {
        eprintln!(
            "GPU setting: {} measured {}, too small to improve this CPU setup; continuing CPU-only",
            session.device_name(),
            format_rate(measured_rate)
        );
        session.stop_and_join()?;
        return Ok(None);
    }
    eprintln!(
        "GPU setting: {} enabled at a measured {} beside the CPU",
        session.device_name(),
        format_rate(measured_rate)
    );
    Ok(Some(session))
}

fn combined_attempts(
    cpu: &onionloom::search::cpu::SearchSession,
    gpu: Option<&GpuSearchSession>,
) -> u64 {
    cpu.attempts()
        .saturating_add(gpu.map_or(0, GpuSearchSession::attempts))
}

fn prepare_search(
    arguments: &SearchArgs,
    filters: Arc<FilterSet>,
    normalized_patterns: &[String],
) -> Result<PreparedSearch> {
    let duration = Duration::from_millis(arguments.tune_ms);
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
        let baseline =
            measure_filtered_median(checkpoint.workers(), batch_size, duration, filters)?;
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
        duration,
        filters,
        arguments.power.unwrap_or(PowerMode::DEFAULT),
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
    duration: Duration,
    filters: Arc<FilterSet>,
    power: PowerMode,
) -> Result<(usize, BatchSize, Measurement)> {
    match (workers, batch_size) {
        (Some(workers), Some(batch)) => Ok((
            workers,
            batch,
            measure_filtered_median(workers, batch, duration, filters)?,
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
            let candidate = fastest(&measurements);
            let confirmed =
                measure_filtered_median(workers, candidate.batch_size, duration, filters)?;
            Ok((workers, candidate.batch_size, confirmed))
        }
        (None, Some(batch)) => {
            let available = std::thread::available_parallelism()?.get();
            let worker_limit = power.worker_limit(available);
            let mut measurements = Vec::new();
            for workers in 1..=worker_limit {
                measurements.push(measure_filtered(
                    workers,
                    batch,
                    duration,
                    Arc::clone(&filters),
                )?);
            }
            let candidate = fastest(&measurements);
            let confirmed = measure_filtered_median(candidate.workers, batch, duration, filters)?;
            Ok((candidate.workers, batch, confirmed))
        }
        (None, None) => {
            let available = std::thread::available_parallelism()?.get();
            let worker_limit = power.worker_limit(available);
            eprintln!(
                "Checking this computer to choose the best measured setup within {} power mode...",
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

fn fastest(measurements: &[Measurement]) -> Measurement {
    *measurements
        .iter()
        .max_by(|left, right| {
            left.attempts_per_second
                .total_cmp(&right.attempts_per_second)
        })
        .expect("measurement options are non-empty")
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

fn remove_completed_checkpoint(path: Option<&Path>) -> Result<()> {
    if let Some(path) = path.filter(|path| path.exists()) {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove completed checkpoint {}", path.display()))?;
    }
    Ok(())
}

fn report_progress(attempts: u64, elapsed: Duration, probability: f64) {
    let rate = attempts as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let estimate = estimates(probability, rate);
    eprintln!(
        "Tested {} possible addresses | {} | 50% ETA {} | 95% ETA {}",
        format_count(attempts as f64),
        format_rate(rate),
        format_duration(estimate.median),
        format_duration(estimate.p95)
    );
}
