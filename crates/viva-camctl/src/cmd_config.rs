//! `save` and `load`: a camera's configuration to a file and back, through
//! [`Camera::save_features`] and [`Camera::load_features`] (ADR-0028).

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use viva_genicam::Camera;
use viva_genicam::genapi::{Configuration, LoadReport, RegisterIo};
use viva_gige::IfaceSelector;

use crate::common::{self, Target};
use crate::with_camera;

/// Save `camera`'s configuration to `path`.
pub fn save<T: RegisterIo>(camera: &mut Camera<T>, path: &Path) -> Result<Configuration> {
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut out = BufWriter::new(file);
    let saved = camera
        .save_features(&mut out)
        .with_context(|| format!("save the configuration to {}", path.display()))?;
    out.flush()?;
    Ok(saved)
}

/// Load the configuration in `path` onto `camera`.
pub fn load<T: RegisterIo>(camera: &mut Camera<T>, path: &Path) -> Result<LoadReport> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    camera
        .load_features(&mut BufReader::new(file))
        .with_context(|| format!("load the configuration in {}", path.display()))
}

/// What a load did: the warnings, each value that failed and why, and the
/// totals.
pub fn render_load_report(report: &LoadReport, out: &mut dyn Write) -> std::io::Result<()> {
    for warning in &report.warnings {
        writeln!(out, "warning: {warning}")?;
    }
    for failure in &report.failures {
        writeln!(out, "not written: {failure}")?;
    }
    write!(out, "{} values written", report.written)?;
    if report.written_on_retry > 0 {
        write!(
            out,
            " ({} on the second pass, once what they depend on was in place)",
            report.written_on_retry
        )?;
    }
    writeln!(out, ", {} not written", report.failures.len())
}

#[derive(Serialize)]
struct SaveSummary<'a> {
    file: &'a str,
    values: usize,
    left_out: Vec<String>,
}

#[derive(Serialize)]
struct LoadSummary {
    written: usize,
    written_on_retry: usize,
    failures: Vec<String>,
    warnings: Vec<String>,
}

pub async fn run_save(
    target: Target,
    iface: Option<IfaceSelector>,
    out: &Path,
    json: bool,
) -> Result<()> {
    // A save sets each selector to every entry in turn, so it writes.
    let mut camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Control)
        .await
        .context("open camera for save")?;
    let saved = with_camera!(&mut camera, camera => save(camera, out))?;
    if json {
        return common::print_json(&SaveSummary {
            file: &out.to_string_lossy(),
            values: saved.values.len(),
            left_out: saved
                .omitted
                .iter()
                .map(|o| format!("{}: {}", o.feature, o.reason))
                .collect(),
        });
    }
    println!(
        "saved {} values to {}; {} left out (the file's comments say why)",
        saved.values.len(),
        out.display(),
        saved.omitted.len()
    );
    Ok(())
}

pub async fn run_load(
    target: Target,
    iface: Option<IfaceSelector>,
    file: &Path,
    json: bool,
) -> Result<()> {
    let mut camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Control)
        .await
        .context("open camera for load")?;
    let report = with_camera!(&mut camera, camera => load(camera, file))?;
    if json {
        common::print_json(&LoadSummary {
            written: report.written,
            written_on_retry: report.written_on_retry,
            failures: report.failures.iter().map(ToString::to_string).collect(),
            warnings: report.warnings.clone(),
        })?;
    } else {
        render_load_report(&report, &mut std::io::stdout().lock())?;
    }
    if !report.is_complete() {
        bail!(
            "{} values in {} could not be written",
            report.failures.len(),
            file.display()
        );
    }
    Ok(())
}
