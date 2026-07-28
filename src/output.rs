//! Validated, permission-restricted Tor key directory output.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

#[cfg(unix)]
use std::fs::File;

use crate::address::{onion_address, validate_address};
use crate::filter::FilterSet;
use crate::keys::{ExpandedSecret, parse_tagged_public_key, tagged_public_key};

/// A key directory validated from disk.
#[derive(Debug)]
pub struct VerifiedDirectory {
    /// Validated onion hostname.
    pub hostname: String,
    /// Compressed Ed25519 public key.
    pub public_key: [u8; 32],
}

/// Validates key relationships and writes a new Tor key directory.
pub fn save_key(
    output_root: &Path,
    secret: &ExpandedSecret,
    public: &[u8; 32],
    filters: &FilterSet,
) -> Result<PathBuf> {
    validate_material(secret, public, Some(filters))?;
    fs::create_dir_all(output_root).with_context(|| {
        format!(
            "failed to create output directory {}",
            output_root.display()
        )
    })?;

    let hostname = onion_address(public);
    let destination = output_root.join(&hostname);
    ensure!(
        !destination.exists(),
        "refusing to overwrite existing key directory {}",
        destination.display()
    );

    let temporary = output_root.join(format!(
        ".onionloom-{}-{}.tmp",
        std::process::id(),
        hostname.trim_end_matches(".onion")
    ));
    ensure!(
        !temporary.exists(),
        "stale temporary directory exists: {}",
        temporary.display()
    );
    create_private_directory(&temporary)?;

    let result = (|| -> Result<()> {
        write_new_file(
            &temporary.join("hs_ed25519_secret_key"),
            &secret.tagged_bytes(),
        )?;
        write_new_file(
            &temporary.join("hs_ed25519_public_key"),
            &tagged_public_key(public),
        )?;
        write_new_file(
            &temporary.join("hostname"),
            format!("{hostname}\n").as_bytes(),
        )?;
        let verified = verify_directory(&temporary)?;
        ensure!(verified.hostname == hostname, "saved hostname changed");
        ensure!(verified.public_key == *public, "saved public key changed");
        sync_directory(&temporary)?;
        fs::rename(&temporary, &destination).with_context(|| {
            format!(
                "failed to atomically install key directory {}",
                destination.display()
            )
        })?;
        sync_directory(output_root)?;
        Ok(())
    })();

    if result.is_err() && temporary.exists() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result?;
    Ok(destination)
}

/// Reads a Tor key directory and validates every relationship.
pub fn verify_directory(directory: &Path) -> Result<VerifiedDirectory> {
    let secret_path = directory.join("hs_ed25519_secret_key");
    let public_path = directory.join("hs_ed25519_public_key");
    let hostname_path = directory.join("hostname");

    let secret_bytes = fs::read(&secret_path)
        .with_context(|| format!("failed to read {}", secret_path.display()))?;
    let public_bytes = fs::read(&public_path)
        .with_context(|| format!("failed to read {}", public_path.display()))?;
    let hostname = fs::read_to_string(&hostname_path)
        .with_context(|| format!("failed to read {}", hostname_path.display()))?;
    ensure!(
        hostname.ends_with('\n') && !hostname[..hostname.len() - 1].contains('\n'),
        "hostname file must contain exactly one newline-terminated hostname"
    );
    let hostname = hostname.trim_end_matches('\n').to_owned();

    let secret = ExpandedSecret::from_tagged_bytes(&secret_bytes)?;
    let public = parse_tagged_public_key(&public_bytes)?;
    let address_public = validate_address(&hostname)?;
    validate_material(&secret, &public, None)?;
    ensure!(
        address_public == public,
        "hostname does not encode the saved public key"
    );
    verify_secret_permissions(&secret_path)?;
    Ok(VerifiedDirectory {
        hostname,
        public_key: public,
    })
}

fn validate_material(
    secret: &ExpandedSecret,
    public: &[u8; 32],
    filters: Option<&FilterSet>,
) -> Result<()> {
    ensure!(
        secret.public_key() == *public,
        "public key does not match expanded secret key"
    );
    if let Some(filters) = filters {
        ensure!(
            filters.matches(public),
            "candidate public key does not match any requested pattern"
        );
    }
    validate_address(&onion_address(public))?;
    Ok(())
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder
        .create(path)
        .with_context(|| format!("failed to create {}", path.display()))
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path).with_context(|| format!("failed to create {}", path.display()))
}

fn write_new_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn verify_secret_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    ensure!(
        mode & 0o077 == 0,
        "secret key permissions are too broad: expected no group or other access"
    );
    Ok(())
}

#[cfg(not(unix))]
fn verify_secret_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::filter::{FilterSet, Pattern};
    use crate::keys::ExpandedSecret;

    use super::{save_key, verify_directory};

    #[test]
    fn saves_and_reloads_exact_key_directory() {
        let root = tempfile::tempdir().expect("temporary directory");
        let secret = ExpandedSecret::generate().expect("operating-system randomness");
        let public = secret.public_key();
        let address = crate::address::onion_address(&public);
        let filter = FilterSet::new(vec![
            Pattern::parse(&address[..1]).expect("valid one-character prefix"),
        ])
        .expect("filter set");
        let directory = save_key(root.path(), &secret, &public, &filter).expect("save key");
        let verified = verify_directory(&directory).expect("verify key");
        assert_eq!(verified.public_key, public);
    }
}
