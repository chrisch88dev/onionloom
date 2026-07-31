//! Detailed CPU benchmark command.

use std::time::Duration;

use anyhow::{Result, ensure};
use clap::Args;

use onionloom::performance::tune::{Measurement, auto_tune, measure};
use onionloom::search::cpu::BatchSize;

use crate::cpu::{HardwareInfo, PowerMode};
use crate::helpers::{format_count, format_rate};

#[derive(Debug, Args)]
pub(crate) struct BenchmarkArgs {
    /// CPU range to measure: low, medium, or full. The default is full.
    #[arg(long, value_enum, conflicts_with = "workers")]
    power: Option<PowerMode>,

    /// Exact parallel search task count. Omit to test the selected power range.
    #[arg(short = 't', long, help_heading = "Advanced")]
    workers: Option<usize>,

    /// Candidate batch size. Omit to test every supported size.
    #[arg(long, help_heading = "Advanced")]
    batch_size: Option<usize>,

    /// Milliseconds per timed sample.
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    trial_ms: u64,
}

pub(crate) fn run(arguments: BenchmarkArgs) -> Result<()> {
    let hardware = HardwareInfo::detect()?;
    let duration = Duration::from_millis(arguments.trial_ms);
    let requested_batch = arguments.batch_size.map(BatchSize::parse).transpose()?;
    let power = arguments.power.unwrap_or(PowerMode::DEFAULT);

    println!("Onionloom CPU benchmark");
    println!();
    hardware.print();
    println!();
    println!("Measurement");
    println!("  Time per sample: {:.3} seconds", duration.as_secs_f64());
    println!(
        "  Worker range: up to {} ({})",
        power.worker_limit(hardware.available_processors),
        power.label()
    );
    println!("  No onion key is generated or written.");
    println!(
        "  Rates cover paired-coordinate generation, batch division, and prefix filtering before key-file output."
    );

    match (arguments.workers, requested_batch) {
        (Some(workers), Some(batch)) => {
            ensure!(workers > 0, "worker count must be greater than zero");
            let measurement = measure(workers, batch, duration)?;
            print_table("Requested setup", &[measurement]);
            print_selection("Measured setup", measurement, false);
        }
        (Some(workers), None) => {
            ensure!(workers > 0, "worker count must be greater than zero");
            let mut measurements = Vec::with_capacity(BatchSize::ALL.len());
            for batch in BatchSize::ALL {
                measurements.push(measure(workers, batch, duration)?);
            }
            print_table("Candidate batch-size comparison", &measurements);
            print_selection("Best result in these trials", fastest(&measurements), false);
        }
        (None, Some(batch)) => {
            let mut measurements =
                Vec::with_capacity(power.worker_limit(hardware.available_processors));
            for workers in 1..=power.worker_limit(hardware.available_processors) {
                measurements.push(measure(workers, batch, duration)?);
            }
            print_table("Parallel worker comparison", &measurements);
            print_selection("Best result in these trials", fastest(&measurements), false);
        }
        (None, None) => {
            let tuning = auto_tune(
                duration,
                None,
                power.worker_limit(hardware.available_processors),
            )?;
            print_table(
                "Candidate batch-size comparison",
                &tuning.batch_measurements,
            );
            print_table("Parallel worker comparison", &tuning.worker_measurements);
            print_selection("Recommended measured setup", tuning.selected, true);
        }
    }

    println!();
    println!("How to read this");
    println!("  More addresses per second means a shorter average wait for the same pattern.");
    println!(
        "  Batch size is the paired work grouped together; each batch also tests its center point."
    );
    println!(
        "  Worker scaling can flatten when CPU cores share power, cache, or execution resources."
    );
    println!(
        "  Rates vary with temperature, power policy, background work, and system scheduling."
    );
    println!(
        "  Full power changes no clock or voltage settings; system safety limits stay active."
    );
    println!("  Use `./onionloom guess NAME` for an end-user time estimate with the real filter.");
    Ok(())
}

fn print_table(title: &str, measurements: &[Measurement]) {
    println!();
    println!("{title}");
    println!("  Workers  Batch  Addresses tested  Seconds  Rate");
    for measurement in measurements {
        println!(
            "  {:>7}  {:>5}  {:>16}  {:>7.3}  {}",
            measurement.workers,
            measurement.batch_size.get(),
            format_count(measurement.attempts as f64),
            measurement.elapsed.as_secs_f64(),
            format_rate(measurement.attempts_per_second)
        );
    }
}

fn print_selection(title: &str, measurement: Measurement, confirmed: bool) {
    println!();
    println!("{title}");
    println!("  Parallel search tasks: {}", measurement.workers);
    println!(
        "  Paired candidate batch size: {} plus one center address",
        measurement.batch_size.get()
    );
    println!(
        "  Candidate rate: {}",
        format_rate(measurement.attempts_per_second)
    );
    if confirmed {
        println!("  Rate confirmation: median of three timed samples");
    } else {
        println!("  Rate confirmation: one timed sample");
    }
}

fn fastest(measurements: &[Measurement]) -> Measurement {
    *measurements
        .iter()
        .max_by(|left, right| {
            left.attempts_per_second
                .total_cmp(&right.attempts_per_second)
        })
        .expect("measurement list is non-empty")
}
