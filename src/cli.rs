//! Root command parser and dispatcher.

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

use crate::commands::benchmark::BenchmarkArgs;
use crate::commands::evaluate::EvaluateArgs;
use crate::commands::gpu::GpuArgs;
use crate::commands::guess::GuessArgs;
use crate::commands::search::SearchArgs;
use crate::commands::verify::VerifyArgs;

#[derive(Debug, Parser)]
#[command(
    name = "onionloom",
    disable_help_flag = true,
    disable_version_flag = true,
    about = "Create custom Tor v3 onion addresses",
    long_about = "Create custom Tor v3 onion addresses with measured estimates and safe key output.\n\nOnionloom measures real candidate generation on this computer, selects an efficient full-power CPU setup, validates compatible GPU hardware, and explains the probability and expected wait before searching. Unsupported or unhelpful GPUs fall back to CPU automatically. It never needs a network connection.",
    after_help = "START HERE:\n  Understand this computer:  ./onionloom evaluate\n  Estimate a name:           ./onionloom guess myname\n  Generate a key:            ./onionloom search myname\n  Inspect GPU acceleration:  ./onionloom gpu\n  Validate generated keys:   ./onionloom verify\n\nLEARN MORE:\n  Explain any command:       ./onionloom help COMMAND"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate and safely save custom onion keys.
    #[command(disable_help_flag = true)]
    Search(SearchArgs),
    /// Measure this computer and explain realistic search times.
    #[command(visible_alias = "eval", disable_help_flag = true)]
    Evaluate(EvaluateArgs),
    /// Estimate the wait for a name without generating a key.
    #[command(visible_alias = "estimate", disable_help_flag = true)]
    Guess(GuessArgs),
    /// Inspect CPU scaling and candidate-generation performance.
    #[command(disable_help_flag = true)]
    Benchmark(BenchmarkArgs),
    /// Inspect available GPU acceleration.
    #[command(disable_help_flag = true)]
    Gpu(GpuArgs),
    /// Validate one generated key directory or an entire output directory.
    #[command(disable_help_flag = true)]
    Verify(VerifyArgs),
    /// Print the Onionloom version.
    Version,
}

pub(crate) fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Search(arguments)) => crate::commands::search::run(arguments),
        Some(Command::Evaluate(arguments)) => crate::commands::evaluate::run(arguments),
        Some(Command::Guess(arguments)) => crate::commands::guess::run(arguments),
        Some(Command::Benchmark(arguments)) => crate::commands::benchmark::run(arguments),
        Some(Command::Gpu(arguments)) => crate::commands::gpu::run(arguments),
        Some(Command::Verify(arguments)) => crate::commands::verify::run(arguments),
        Some(Command::Version) => {
            println!("Onionloom {}", env!("CARGO_PKG_VERSION"));
            println!("{}", env!("ONIONLOOM_COPYRIGHT"));
            println!("Owner: {}", env!("ONIONLOOM_OWNER"));
            println!("Website: {}", env!("ONIONLOOM_WEBSITE"));
            println!("Contact: {}", env!("ONIONLOOM_CONTACT"));
            println!("Engines: {}", env!("ONIONLOOM_ENGINES"));
            Ok(())
        }
        None => {
            Cli::command().print_long_help()?;
            println!();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command};
    use crate::commands::search::GpuMode;
    use crate::cpu::PowerMode;

    #[test]
    fn root_uses_help_and_version_commands_without_root_flags() {
        let mut command = Cli::command();
        command.build();
        assert!(command.get_arguments().all(|argument| {
            argument.get_long() != Some("help") && argument.get_long() != Some("version")
        }));
        assert!(command.find_subcommand("help").is_some());
        assert!(command.find_subcommand("version").is_some());
        for name in ["search", "evaluate", "guess", "benchmark", "gpu", "verify"] {
            let subcommand = command
                .find_subcommand(name)
                .expect("declared subcommand exists");
            assert!(
                subcommand
                    .get_arguments()
                    .all(|argument| argument.get_long() != Some("help"))
            );
        }
    }

    #[test]
    fn guess_alias_and_power_control_parse() {
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
    fn continuous_search_uses_a_distinct_count_mode() {
        let cli = Cli::try_parse_from(["onionloom", "search", "loom", "--continuous"])
            .expect("valid continuous command");
        let Some(Command::Search(arguments)) = cli.command else {
            panic!("search command expected");
        };
        assert!(arguments.continuous);
        assert_eq!(arguments.count, None);

        let conflicting = Cli::try_parse_from([
            "onionloom",
            "search",
            "loom",
            "--continuous",
            "--count",
            "2",
        ]);
        assert!(conflicting.is_err());
    }

    #[test]
    fn search_uses_automatic_gpu_policy_by_default() {
        let cli =
            Cli::try_parse_from(["onionloom", "search", "longname"]).expect("valid search command");
        let Some(Command::Search(arguments)) = cli.command else {
            panic!("search command expected");
        };
        assert_eq!(arguments.gpu, GpuMode::Auto);

        let cli = Cli::try_parse_from(["onionloom", "search", "longname", "--gpu", "off"])
            .expect("valid CPU-only search command");
        let Some(Command::Search(arguments)) = cli.command else {
            panic!("search command expected");
        };
        assert_eq!(arguments.gpu, GpuMode::Off);
    }
}
