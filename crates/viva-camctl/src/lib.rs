//! `viva-camctl` — the GenICam diagnostic CLI, as a library.
//!
//! The command-line surface lives in a private `cli` module, driven by [`run`], so there
//! is exactly one implementation of it. The `viva-camctl` binary calls [`run`],
//! and so does the `viva-camctl` console script that `pip install viva-genicam`
//! installs — the Python users we most often ask for a `viva-camctl report` are
//! the ones least able to build it from source, and a second entry point would
//! be a second thing to keep in step.

mod cli;
mod cmd_action;
mod cmd_bench;
mod cmd_chunks;
mod cmd_config;
mod cmd_events;
mod cmd_execute;
mod cmd_get;
mod cmd_inspect;
mod cmd_list;
mod cmd_registers;
mod cmd_report;
mod cmd_set;
mod cmd_set_ip;
mod cmd_stream;
mod cmd_usb;
mod cmd_xml;
mod common;
#[cfg(test)]
mod field_engineer;

use std::ffi::OsString;

use common::with_camera;

use clap::Parser;

/// Exit code for a command that ran but failed, matching `ExitCode::FAILURE`.
const FAILURE: u8 = 1;

/// Parse `args`, run the selected command to completion, and return the exit
/// code the process should use.
///
/// `args` is a complete argv, program name included. Diagnostics go to stderr in
/// the same form the binary has always produced — `anyhow`'s `Debug` rendering,
/// so an error's source chain survives. The code is clap's own for a usage error
/// or for `--help`, and 1 for a command that ran and failed.
///
/// A plain `u8` rather than [`ExitCode`](std::process::ExitCode) because
/// `ExitCode` cannot be inspected, and the Python entry point has to hand the
/// number back to the interpreter rather than exit the process itself.
///
/// Builds its own multi-thread tokio runtime rather than requiring one, so it
/// works from a plain `fn main` and from a Python interpreter that has none.
pub fn run<I, T>(args: I) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match cli::Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            // Covers `--help` and `--version` too, which clap reports as errors
            // carrying an exit code of 0.
            let _ = err.print();
            return u8::try_from(err.exit_code()).unwrap_or(FAILURE);
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("Error: could not start the async runtime: {err}");
            return FAILURE;
        }
    };

    match runtime.block_on(cli::dispatch(cli)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("Error: {err:?}");
            FAILURE
        }
    }
}
