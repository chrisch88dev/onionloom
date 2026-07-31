//! Hardware evaluation command.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Args;

use onionloom::performance::eta::{estimates, format_duration};
use onionloom::performance::tune::auto_tune;
use onionloom::search::filter::{FilterSet, Pattern};

use crate::cpu::{HardwareInfo, PowerMode};
use crate::helpers::{
    automatic_gpu_is_worth_starting, automatic_gpu_rate_is_useful, format_rate,
    measure_available_gpu, print_power_mode,
};

#[derive(Debug, Args)]
pub(crate) struct EvaluateArgs {
    /// CPU use: low, medium, or full. The default is full.
    #[arg(long, value_enum)]
    power: Option<PowerMode>,

    /// Milliseconds spent measuring each setup.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u64).range(25..), help_heading = "Advanced")]
    trial_ms: u64,
}

pub(crate) fn run(arguments: EvaluateArgs) -> Result<()> {
    let hardware = HardwareInfo::detect()?;
    let power = arguments.power.unwrap_or(PowerMode::DEFAULT);
    let worker_limit = power.worker_limit(hardware.available_processors);

    println!("Onionloom hardware evaluation");
    println!();
    hardware.print();
    if onionloom::gpu::compiled() {
        for device in onionloom::gpu::devices() {
            let kind = if device.discrete {
                "discrete"
            } else {
                "integrated"
            };
            println!("  GPU: {} ({kind}, {})", device.name, device.backend);
        }
    }
    println!();
    println!("Measuring real candidate generation on this computer...");
    let tuning = auto_tune(
        Duration::from_millis(arguments.trial_ms),
        None,
        worker_limit,
    )?;

    println!();
    println!("Recommended setup");
    print_power_mode(power, hardware.available_processors);
    println!("Parallel search tasks: {}", tuning.selected.workers);
    println!(
        "Paired candidate batch size: {} plus one center address",
        tuning.selected.batch_size.get()
    );
    println!(
        "Measured CPU candidate rate: {}",
        format_rate(tuning.selected.attempts_per_second)
    );
    println!("The selected rate is the median of three timed confirmation samples.");
    let gpu_filters = Arc::new(FilterSet::new(vec![Pattern::parse("zzzzzzzzzz")?])?);
    let gpu = measure_available_gpu(gpu_filters).filter(|measurement| {
        automatic_gpu_rate_is_useful(
            tuning.selected.attempts_per_second,
            measurement.attempts_per_second,
        )
    });
    if let Some(gpu) = gpu.as_ref() {
        println!(
            "Measured GPU candidate rate: {} ({})",
            format_rate(gpu.attempts_per_second),
            gpu.device_name
        );
        println!(
            "Estimated combined candidate rate: {}",
            format_rate(tuning.selected.attempts_per_second + gpu.attempts_per_second)
        );
    } else {
        println!("Automatic GPU use: no compatible useful adapter measured");
    }

    println!();
    println!("Estimated time for one exact address beginning");
    println!("Characters  Engines  Average time  50% chance by  95% chance by");
    for length in 3..=10 {
        let probability = 2_f64.powi(-5 * length);
        let difficulty = 1.0 / probability;
        let use_gpu = gpu.is_some()
            && automatic_gpu_is_worth_starting(difficulty, tuning.selected.attempts_per_second);
        let selected_rate = tuning.selected.attempts_per_second
            + if use_gpu {
                gpu.as_ref()
                    .map_or(0.0, |measurement| measurement.attempts_per_second)
            } else {
                0.0
            };
        let estimate = estimates(probability, selected_rate);
        println!(
            "{length:>10}  {:>7}  {:>12}  {:>13}  {:>13}",
            if use_gpu { "CPU+GPU" } else { "CPU" },
            format_duration(estimate.expected),
            format_duration(estimate.median),
            format_duration(estimate.p95)
        );
    }

    println!();
    println!("How to read this");
    println!("  Average is the long-run mean, not a deadline.");
    println!("  50% means half of comparable searches should finish by that time.");
    println!("  95% means 95 out of 100 comparable searches should finish by that time.");
    println!(
        "  Estimates use measured work, so CPU design, available cores, current clock behavior,"
    );
    println!(
        "  power limits, temperature, system load, and operating-system scheduling are included."
    );
    println!(
        "  Full power changes no clock or voltage settings; system safety limits stay active."
    );
    println!("  GPU is included only where its measured rate can reasonably repay startup.");
    println!("  Run `./onionloom guess NAME` for a specific pattern.");
    Ok(())
}
