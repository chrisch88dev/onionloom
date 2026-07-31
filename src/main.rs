//! Onionloom command-line entry point.

mod cli;
mod commands;
mod cpu;
mod helpers;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
