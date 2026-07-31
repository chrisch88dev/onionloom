//! Pattern estimate command.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Args;

use onionloom::performance::eta::estimates;
use onionloom::performance::tune::auto_tune;
use onionloom::search::filter::Pattern;

use crate::cpu::{HardwareInfo, PowerMode};
use crate::helpers::{
    automatic_gpu_is_worth_starting, automatic_gpu_rate_is_useful, format_count, format_rate,
    measure_available_gpu, parse_filters, print_power_mode, print_time_estimates,
};

#[derive(Debug, Args)]
pub(crate) struct GuessArgs {
    /// Address beginnings to estimate, such as loom or private2.
    ///
    /// Use lowercase letters a-z and digits 2-7. With several patterns,
    /// the estimate is for finding any one of them.
    #[arg(required = true)]
    pub(crate) patterns: Vec<String>,

    /// CPU use: low, medium, or full. The default is full.
    #[arg(long, value_enum)]
    pub(crate) power: Option<PowerMode>,

    /// Milliseconds spent measuring each setup.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    trial_ms: u64,
}

pub(crate) fn run(arguments: GuessArgs) -> Result<()> {
    let filters = parse_filters(&arguments.patterns)?;
    let hardware = HardwareInfo::detect()?;
    let power = arguments.power.unwrap_or(PowerMode::DEFAULT);
    let worker_limit = power.worker_limit(hardware.available_processors);

    println!("Onionloom pattern estimate");
    println!();
    hardware.print();
    println!();
    println!("Measuring the requested pattern on this computer...");
    let tuning = auto_tune(
        Duration::from_millis(arguments.trial_ms),
        Some(Arc::clone(&filters)),
        worker_limit,
    )?;
    let probability = filters.probability();
    let difficulty = 1.0 / probability;
    let gpu = automatic_gpu_is_worth_starting(difficulty, tuning.selected.attempts_per_second)
        .then(|| measure_available_gpu(Arc::clone(&filters)))
        .flatten()
        .filter(|measurement| {
            automatic_gpu_rate_is_useful(
                tuning.selected.attempts_per_second,
                measurement.attempts_per_second,
            )
        });
    let selected_rate = tuning.selected.attempts_per_second
        + gpu
            .as_ref()
            .map_or(0.0, |measurement| measurement.attempts_per_second);
    let estimate = estimates(probability, selected_rate);

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
    print_power_mode(power, hardware.available_processors);
    println!(
        "Recommended parallel search tasks: {}",
        tuning.selected.workers
    );
    println!(
        "Recommended paired batch size: {} plus one center address",
        tuning.selected.batch_size.get()
    );
    println!(
        "Measured CPU candidate rate: {}",
        format_rate(tuning.selected.attempts_per_second)
    );
    if let Some(gpu) = gpu {
        println!(
            "Measured GPU candidate rate: {} ({})",
            format_rate(gpu.attempts_per_second),
            gpu.device_name
        );
        println!(
            "Estimated combined candidate rate: {}",
            format_rate(selected_rate)
        );
    } else {
        println!("Automatic GPU use: not useful or unavailable for this estimate");
    }
    println!(
        "Difficulty: about 1 match per {} possible addresses tested",
        format_count(1.0 / probability)
    );
    print_time_estimates(estimate);
    println!();
    println!("The selected rate is the median of three timed confirmation samples.");
    println!(
        "This estimate includes the current CPU, available cores, power and thermal behavior,"
    );
    println!("system load, operating-system scheduling, and the real compiled pattern filter.");
    println!("Full power changes no clock or voltage settings; system safety limits stay active.");
    println!("It is a probability estimate, not a completion deadline.");
    Ok(())
}
