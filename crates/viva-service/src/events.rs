//! GenICam device events over Zenoh, for every [`StreamSource`]: the
//! `events/control` queryable starts and stops monitoring, each event is
//! published on `events` with the features its XML reads from it, and
//! `events/status` says when monitoring starts and stops.
//!
//! Starting changes the camera's event settings, and stopping puts back what
//! starting changed ([`viva_genicam::Camera::restore_events`]) — on a stop
//! request, when the camera is let go of, and when the event channel fails.
//! What could not be put back is reported, never passed off as restored.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{info, warn};
use viva_genicam::EventConfiguration;
use viva_zenoh_api::{
    CameraEvent, EventCommand, EventControlRequest, EventMonitorStatus, keys, restore_warnings,
};
use zenoh::Session;

use crate::source::{EventReceiver, StreamSource};

/// What the loop forwarding events hands back when it ends: the receiver to
/// close, and why it ended by itself, if it did.
type Ended = (Box<dyn EventReceiver>, Option<String>);

/// Monitoring that is on.
struct Monitor {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Ended>,
    saved: EventConfiguration,
}

/// Answer the event control queryable until `shutdown`, then stop any
/// monitoring and restore the camera's event settings.
pub(crate) async fn run<D: StreamSource>(
    session: Arc<Session>,
    device: Arc<D>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::events_control(&device_id);
    let status_key = keys::events_status(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare the event control queryable");
            return;
        }
    };
    info!(device_id, key, "event control queryable ready");
    let mut monitor: Option<Monitor> = None;

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                let Ok(query) = query else { break };
                let request = query
                    .payload()
                    .ok_or_else(|| "missing payload".to_string())
                    .and_then(|payload| {
                        serde_json::from_slice::<EventControlRequest>(&payload.to_bytes())
                            .map_err(|e| format!("invalid payload: {e}"))
                    });
                let reply = match request {
                    Ok(request) => match request.command {
                        EventCommand::Start => {
                            let mut warnings = Vec::new();
                            if let Some(previous) = monitor.take() {
                                warnings = stop(device.as_ref(), previous).await.warnings;
                            }
                            start(&session, &device, request.events)
                                .await
                                .map(|(started, events)| {
                                    monitor = Some(started);
                                    let mut status = EventMonitorStatus::active(events);
                                    status.warnings = warnings;
                                    status
                                })
                        }
                        EventCommand::Stop => Ok(match monitor.take() {
                            Some(running) => stop(device.as_ref(), running).await,
                            None => EventMonitorStatus::inactive(Vec::new()),
                        }),
                    },
                    Err(message) => Err(message),
                };
                match reply {
                    Ok(status) => {
                        put_status(&session, &status_key, &status).await;
                        if let Ok(payload) = serde_json::to_vec(&status) {
                            let _ = query.reply(&key, payload).await;
                        }
                    }
                    Err(message) => {
                        warn!(device_id, %message, "event control refused");
                        let _ = query.reply_err(message).await;
                    }
                }
            }
            // The event channel failed: undo what starting changed and say so.
            ended = async {
                match monitor.as_mut() {
                    Some(running) => (&mut running.task).await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(running) = monitor.take() else { continue };
                let status = finish(device.as_ref(), running.saved, ended).await;
                put_status(&session, &status_key, &status).await;
            }
            _ = crate::serve::stopped(&mut shutdown) => {
                if let Some(running) = monitor.take() {
                    let status = stop(device.as_ref(), running).await;
                    put_status(&session, &status_key, &status).await;
                }
                break;
            }
        }
    }
}

/// Configure the camera, open the channel, and forward its events.
async fn start<D: StreamSource>(
    session: &Arc<Session>,
    device: &Arc<D>,
    events: Vec<String>,
) -> Result<(Monitor, Vec<String>), String> {
    if events.is_empty() {
        return Err("Choose at least one event to monitor".into());
    }
    let (receiver, saved) = device.open_events(events.clone()).await?;
    let (stop, stop_rx) = watch::channel(false);
    let task = tokio::spawn(forward(session.clone(), device.clone(), receiver, stop_rx));
    info!(
        device_id = device.device_id(),
        ?events,
        "event monitoring started"
    );
    Ok((Monitor { stop, task, saved }, events))
}

/// Publish each event until `stop`, or until the channel fails.
async fn forward<D: StreamSource>(
    session: Arc<Session>,
    device: Arc<D>,
    receiver: Box<dyn EventReceiver>,
    mut stop: watch::Receiver<bool>,
) -> Ended {
    let key = keys::events(device.device_id());
    let blocks = device.events_carry_blocks();
    loop {
        let event = tokio::select! {
            _ = crate::serve::stopped(&mut stop) => return (receiver, None),
            event = receiver.next_event() => match event {
                Ok(event) => event,
                Err(error) => return (receiver, Some(error.to_string())),
            },
        };
        let wire = CameraEvent::new(&event, !blocks);
        let read = device
            .camera()
            .with(move |camera| Ok((camera.event_name(event.id), camera.event_features(&event))))
            .await;
        let wire = match read {
            Ok((name, features)) => wire.with_name(name).with_features(features),
            Err(error) => wire.with_features([("Event".to_string(), Err::<String, _>(error))]),
        };
        if let Ok(payload) = serde_json::to_vec(&wire) {
            let _ = session.put(&key, payload).await;
        }
    }
}

/// Stop forwarding, then restore.
async fn stop<D: StreamSource>(device: &D, monitor: Monitor) -> EventMonitorStatus {
    let _ = monitor.stop.send(true);
    let ended = monitor.task.await;
    finish(device, monitor.saved, ended).await
}

/// Close the receiver the forwarding loop handed back and restore the
/// settings starting changed, collecting what could not be undone.
async fn finish<D: StreamSource>(
    device: &D,
    saved: EventConfiguration,
    ended: Result<Ended, tokio::task::JoinError>,
) -> EventMonitorStatus {
    let mut warnings = Vec::new();
    match ended {
        Ok((receiver, reason)) => {
            if let Some(reason) = reason {
                warnings.push(format!("Event monitoring stopped: {reason}"));
            }
            if let Err(error) = receiver.close().await {
                warnings.push(format!("The event channel was not closed cleanly: {error}"));
            }
        }
        Err(error) => warnings.push(format!("Event monitoring stopped: {error}")),
    }
    match device
        .camera()
        .with(move |camera| Ok(camera.restore_events(saved)))
        .await
    {
        Ok(report) => warnings.extend(restore_warnings(&report)),
        Err(error) => warnings.push(format!("Event settings were not restored: {error}")),
    }
    if !warnings.is_empty() {
        warn!(
            device_id = device.device_id(),
            ?warnings,
            "event monitoring stopped"
        );
    }
    EventMonitorStatus::inactive(warnings)
}

async fn put_status(session: &Session, key: &str, status: &EventMonitorStatus) {
    if let Ok(payload) = serde_json::to_vec(status) {
        let _ = session.put(key, payload).await;
    }
}
