//! CI builder for the model capability catalogue.
//!
//! `pages.yml` runs this as a deploy step: the three public feeds are
//! downloaded, merged (`model_catalog::build_catalog`) and written as
//! `model_metadata.json`, which the Pages deployment publishes and the app
//! fetches directly. This crate exists so the merge never has to run inside
//! the app — see its crate docs.
//!
//! Usage: `build-catalog [<output-path>]`
//!
//! With no argument (or `-`) the catalogue is written to stdout. The process
//! exits non-zero when the merge failed or the result fails
//! [`model_catalog::ensure_usable`] — a deploy must not publish a stub.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    // `-` means stdout, like most CLI tools.
    let output = std::env::args().nth(1).filter(|path| path != "-");
    match run(output.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("build-catalog: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(output: Option<&str>) -> Result<(), String> {
    let catalog = model_catalog::build_catalog()?;
    model_catalog::ensure_usable(&catalog)
        .map_err(|err| format!("the merged catalogue is not usable: {err}"))?;

    let bytes = serde_json::to_vec(&catalog)
        .map_err(|err| format!("cannot serialize the catalogue: {err}"))?;

    match output {
        Some(path) => {
            // Atomic write: a deploy step that dies halfway must not leave a
            // truncated file for the upload step to pick up.
            let path = Path::new(path);
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, &bytes).map_err(|err| format!("cannot write {}: {err}", tmp.display()))?;
            if let Err(err) = std::fs::rename(&tmp, path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("cannot write {}: {err}", path.display()));
            }
            eprintln!(
                "build-catalog: wrote {} entries to {}",
                catalog.entries.len(),
                path.display()
            );
        }
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout
                .write_all(&bytes)
                .map_err(|err| format!("cannot write to stdout: {err}"))?;
        }
    }
    Ok(())
}
