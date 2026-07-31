//! Generated-key verification command.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{Args, ValueHint};

use onionloom::storage::output::{VerifiedDirectory, verify_directory};

#[derive(Debug, Args)]
#[command(
    after_help = "EXAMPLES:\n  Verify every generated key:  ./onionloom verify\n  Verify an output directory:  ./onionloom verify onions\n  Verify one key directory:     ./onionloom verify onions/<press Tab here>"
)]
pub(crate) struct VerifyArgs {
    /// Key directory or output root to validate. Defaults to ./onions.
    #[arg(value_hint = ValueHint::DirPath)]
    directory: Option<PathBuf>,
}

pub(crate) fn run(arguments: VerifyArgs) -> Result<()> {
    let directory = arguments
        .directory
        .unwrap_or_else(|| PathBuf::from("onions"));
    ensure!(
        directory.is_dir(),
        "{} is not a directory; run `./onionloom search NAME` first or choose a directory with Tab completion",
        directory.display()
    );

    if is_key_directory(&directory) {
        let verified = verify_one(&directory)?;
        print_verified(&directory, &verified);
        return Ok(());
    }

    let directories = discover_key_directories(&directory)?;
    ensure!(
        !directories.is_empty(),
        "no .onion key directories found under {}; run `./onionloom search NAME` first or pass a key directory directly",
        directory.display()
    );

    println!(
        "Checking {} generated key director{} under {}",
        directories.len(),
        if directories.len() == 1 { "y" } else { "ies" },
        directory.display()
    );
    for candidate in &directories {
        let verified = verify_one(candidate)?;
        print_verified(candidate, &verified);
    }
    println!();
    println!(
        "Verification complete: all {} directories passed.",
        directories.len()
    );
    Ok(())
}

fn verify_one(directory: &Path) -> Result<VerifiedDirectory> {
    verify_directory(directory)
        .with_context(|| format!("verification failed for {}", directory.display()))
}

fn print_verified(directory: &Path, verified: &VerifiedDirectory) {
    println!();
    println!("Verification passed");
    println!("  Directory: {}", directory.display());
    println!("  Address: {}", verified.hostname);
    println!("  Checked: exact file sizes and headers");
    println!("  Checked: secret key, public key, hostname, and Tor checksum agree");
    #[cfg(unix)]
    println!("  Checked: secret key permissions block group and other access");
}

fn is_key_directory(directory: &Path) -> bool {
    has_onion_extension(directory)
        || ["hostname", "hs_ed25519_public_key", "hs_ed25519_secret_key"]
            .iter()
            .any(|name| directory.join(name).exists())
}

fn discover_key_directories(root: &Path) -> Result<Vec<PathBuf>> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to list output directory {}", root.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read an entry under {}", root.display()))?;
        if entry.file_type()?.is_dir() && has_onion_extension(&entry.path()) {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

fn has_onion_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("onion"))
}
