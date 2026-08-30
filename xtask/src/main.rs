#[cfg(test)]
#[path = "../../distribution/jsc/backend.rs"]
mod jsc_backend;
#[cfg(test)]
#[path = "../../crates/kunlun-jsc/src/ownership.rs"]
mod jsc_ownership;

mod jsc_manifest;

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DEFAULT_MANIFEST: &str = "distribution/jsc/manifest.json";

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let Some(command) = arguments.next() else {
        return Err(usage());
    };
    let Some(action) = arguments.next() else {
        return Err(usage());
    };

    if command != "jsc-manifest" || action != "validate" {
        return Err(usage());
    }

    let repository_root = repository_root();
    let manifest_path = match arguments.next() {
        Some(path) => resolve_from_repository(&repository_root, &path),
        None => repository_root.join(DEFAULT_MANIFEST),
    };
    if arguments.next().is_some() {
        return Err(usage());
    }

    jsc_manifest::validate_file(&manifest_path, &repository_root).map_err(|errors| {
        let details = errors
            .into_iter()
            .map(|error| format!("  - {error}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "JSC distribution manifest validation failed for {}:\n{details}",
            manifest_path.display()
        )
    })?;

    println!(
        "validated JSC distribution manifest: {}",
        manifest_path.display()
    );
    Ok(())
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be located below the repository root")
        .to_path_buf()
}

fn resolve_from_repository(repository_root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repository_root.join(path)
    }
}

fn usage() -> String {
    format!("usage: cargo xtask jsc-manifest validate [{DEFAULT_MANIFEST}]")
}
