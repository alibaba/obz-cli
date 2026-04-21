//! Build script for the `obz` binary crate.
//!
//! Injects compile-time metadata as environment variables so the CLI can
//! display version, commit, build timestamp, and target platform.

use std::process::Command;

fn main() {
    // Git commit hash
    let commit_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let commit_short = if commit_hash.len() >= 7 && commit_hash != "unknown" {
        commit_hash[..7].to_string()
    } else {
        commit_hash.clone()
    };

    // Build timestamp: prefer SOURCE_DATE_EPOCH for reproducible builds,
    // otherwise use the current system time.
    let build_epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        })
        .unwrap_or(0);

    // Target triple (e.g. "x86_64-unknown-linux-gnu")
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=OBZ_COMMIT_HASH={commit_hash}");
    println!("cargo:rustc-env=OBZ_COMMIT_SHORT={commit_short}");
    println!("cargo:rustc-env=OBZ_BUILD_TIMESTAMP={build_epoch}");
    println!("cargo:rustc-env=OBZ_BUILD_TARGET={target}");

    // Re-run when HEAD changes (new commit) or SOURCE_DATE_EPOCH is set.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
}
