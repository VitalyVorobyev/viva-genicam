use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tracing::info;

use viva_genicam::GenicamError;

use viva_gige::IfaceSelector;

use crate::common::{self, Target};
use crate::with_camera;

#[derive(Serialize)]
struct ExecuteResponse<'a> {
    name: &'a str,
    executed: bool,
    /// Whether the camera reported the command finished: `false` with
    /// `--no-wait`, which does not ask, and when it did not report completion
    /// within the wait (the command was still executed).
    done: bool,
}

/// Execute a GenApi `<Command>` feature, then wait for it to finish.
///
/// Separate from `set` because a command has no value: `set --name UserSetLoad
/// --value 1` worked, since `Camera::set` dispatches commands and discards the
/// value, but nothing said so and the required `--value` reads like a mistake
/// (issue #121).
///
/// Waiting is the default because the GenICam Standard (v2.1.1 §2.8.8) asks
/// applications to "always wait for the command completion by calling IsDone
/// immediately after the Execute": a `UserSetLoad` that returns before the
/// camera has loaded the set leaves the next read looking at the old values.
/// For the many commands whose register is write-only, IsDone is true at once
/// and nothing is waited on. `wait: None` (`--no-wait`) returns once the write
/// is acknowledged.
///
/// A camera that has not reported completion when the wait runs out is a
/// warning, not a failure: the command was executed, and no camera has yet
/// shown us whether its command registers clear as the standard expects.
pub async fn run(
    target: Target,
    name: String,
    wait: Option<Duration>,
    iface: Option<IfaceSelector>,
    json: bool,
) -> Result<()> {
    info!(?target, "opening camera for execute");
    let mut camera = common::open_target(&target, iface.as_ref(), viva_genicam::Access::Control)
        .await
        .context("open camera for execute")?;
    let done = with_camera!(&mut camera, camera => execute(camera, &name, wait).await)?;

    if json {
        common::print_json(&ExecuteResponse {
            name: &name,
            executed: true,
            done,
        })?;
    } else if done {
        println!("{name} executed and done");
    } else {
        println!("{name} executed");
    }

    Ok(())
}

/// Execute `name`, and with `wait` wait for it to report done. Whether it
/// did: `false` with no wait, or when it did not finish within the wait.
pub async fn execute<T: viva_genicam::genapi::RegisterIo>(
    camera: &mut viva_genicam::Camera<T>,
    name: &str,
    wait: Option<Duration>,
) -> Result<bool> {
    let name = name.to_string();
    let done = match wait {
        Some(wait) => match camera.execute_and_wait(&name, wait).await {
            Ok(()) => true,
            Err(GenicamError::CommandNotDone { waited, .. }) => {
                eprintln!(
                    "warning: {name} was executed, but the camera did not report completion within {} ms",
                    waited.as_millis()
                );
                false
            }
            Err(err) => return Err(err).with_context(|| format!("execute command {name}")),
        },
        None => {
            camera
                .execute_command(&name)
                .with_context(|| format!("execute command {name}"))?;
            false
        }
    };

    Ok(done)
}
