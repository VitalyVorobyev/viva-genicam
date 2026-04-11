//! USB3 Vision Zenoh service — bridges U3V cameras to genicam-studio.
//!
//! This is the U3V equivalent of `viva-service` (GigE Vision).
//! Supports a `--fake` flag for testing without USB hardware.

mod acquisition;
mod device;

use std::sync::Arc;

use clap::Parser;
use tokio::sync::watch;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use viva_service::device::DeviceOps;
use viva_service::{nodes, status, xml};
use viva_zenoh_api::{API_VERSION, DeviceAnnounce, keys};

use crate::device::U3vDeviceHandle;

#[derive(Parser, Debug)]
#[command(
    name = "viva-service-u3v",
    version,
    about = "USB3 Vision Zenoh service"
)]
struct Cli {
    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Use fake in-process U3V camera (no USB hardware needed)
    #[arg(long)]
    fake: bool,

    /// Fake camera width
    #[arg(long, default_value_t = 640)]
    width: u32,

    /// Fake camera height
    #[arg(long, default_value_t = 480)]
    height: u32,

    /// Pixel format: mono8 or rgb8
    #[arg(long, default_value = "mono8")]
    pixel_format: String,

    /// Path to Zenoh configuration file
    #[arg(long)]
    zenoh_config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    info!("Starting viva-service-u3v");

    let zenoh_config = match &cli.zenoh_config {
        Some(path) => zenoh::Config::from_file(path)?,
        None => zenoh::Config::default(),
    };
    let session = Arc::new(zenoh::open(zenoh_config).await?);
    info!("Zenoh session opened");

    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    if cli.fake {
        let pfnc = match cli.pixel_format.to_lowercase().as_str() {
            "rgb8" | "rgb8packed" => 0x0218_0014u32,
            _ => 0x0108_0001u32, // Mono8
        };
        info!(
            width = cli.width,
            height = cli.height,
            pixel_format = cli.pixel_format,
            "starting fake U3V camera"
        );
        run_fake_camera(
            session.clone(),
            cli.width,
            cli.height,
            pfnc,
            shutdown_rx.clone(),
        )
        .await?;
    } else {
        run_real_camera(session.clone(), shutdown_rx.clone()).await?;
    }

    // Wait for CTRL+C.
    tokio::signal::ctrl_c().await?;
    info!("Shutdown requested (CTRL+C)");
    let _ = _shutdown_tx.send(true);

    // Give tasks a moment to finish.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    session.close().await?;
    info!("viva-service-u3v shut down");
    Ok(())
}

async fn run_fake_camera(
    session: Arc<zenoh::Session>,
    width: u32,
    height: u32,
    pixel_format: u32,
    shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use viva_genicam::open_u3v_device;

    let fake_transport = Arc::new(viva_fake_u3v::FakeU3vTransport::new(
        width,
        height,
        pixel_format,
    ));

    let device =
        viva_u3v::device::U3vDevice::open(fake_transport.clone(), 0x81, 0x01, Some(0x82), None)?;

    let (camera, xml) = open_u3v_device(device)?;

    let device_id = "cam-fake-u3v".to_string();
    let handle = Arc::new(U3vDeviceHandle::new(
        camera,
        xml,
        device_id.clone(),
        fake_transport,
        Some(0x82),
    ));

    // Publish connected status and initial values.
    status::publish_connected(&session, &device_id).await;
    nodes::publish_initial_values(&session, handle.as_ref()).await;

    info!(
        device_id,
        "fake U3V camera connected, spawning service tasks"
    );

    // Spawn shared Zenoh queryables.
    tokio::spawn(xml::run(
        session.clone(),
        device_id.clone(),
        handle.raw_xml().to_string(),
        shutdown.clone(),
    ));
    tokio::spawn(nodes::run_set_queryable(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));
    tokio::spawn(nodes::run_execute_queryable(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));
    tokio::spawn(nodes::run_bulk_read_queryable(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));
    tokio::spawn(acquisition::run(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));

    // Periodic announce so the studio discovers the device even if it
    // starts after the service (studio subscribes to announce topic).
    let announce_session = session.clone();
    let announce_device_id = device_id.clone();
    let mut announce_shutdown = shutdown;
    tokio::spawn(async move {
        let announce = DeviceAnnounce {
            id: announce_device_id.clone(),
            name: "FakeU3V".to_string(),
            model: "FakeU3V".to_string(),
            serial: "FAKE-001".to_string(),
            api_version: Some(API_VERSION),
        };
        let key = keys::announce(&announce_device_id);
        let payload = serde_json::to_vec(&announce).unwrap();
        loop {
            let _ = announce_session.put(&key, payload.clone()).await;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                _ = announce_shutdown.changed() => {
                    if *announce_shutdown.borrow() { break; }
                }
            }
        }
    });

    info!(
        device_id,
        "U3V service tasks spawned (use genicam-studio to connect)"
    );
    Ok(())
}

async fn run_real_camera(
    session: Arc<zenoh::Session>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use viva_genicam::connect_u3v_with_xml;
    use viva_u3v::discovery;

    let devices = discovery::discover().map_err(|e| format!("USB discovery failed: {e}"))?;
    if devices.is_empty() {
        error!("No USB3 Vision cameras found. Use --fake for testing.");
        return Ok(());
    }

    let device_info = &devices[0];
    info!(
        vendor = format!("{:04x}", device_info.vendor_id),
        product = format!("{:04x}", device_info.product_id),
        model = device_info.model.as_deref().unwrap_or("unknown"),
        "connecting to USB3 Vision camera"
    );

    let (camera, xml) = connect_u3v_with_xml(device_info)?;

    // To get the transport Arc and stream endpoint, we need to peek at the
    // RegisterIo's inner device. Since `connect_u3v_with_xml` consumes
    // the device, we reconstruct the transport from the RegisterIo.
    let transport = camera.transport().lock_device()?.transport();
    let stream_ep = camera.transport().lock_device()?.stream_ep();

    let device_id = format!(
        "cam-u3v-{:04x}{:04x}",
        device_info.vendor_id, device_info.product_id
    );

    let handle = Arc::new(U3vDeviceHandle::new(
        camera,
        xml,
        device_id.clone(),
        transport,
        stream_ep,
    ));

    // Publish connected status and initial values.
    status::publish_connected(&session, &device_id).await;
    nodes::publish_initial_values(&session, handle.as_ref()).await;

    info!(
        device_id,
        "USB3 Vision camera connected, spawning service tasks"
    );

    // Spawn shared Zenoh queryables.
    tokio::spawn(xml::run(
        session.clone(),
        device_id.clone(),
        handle.raw_xml().to_string(),
        shutdown.clone(),
    ));
    tokio::spawn(nodes::run_set_queryable(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));
    tokio::spawn(nodes::run_execute_queryable(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));
    tokio::spawn(nodes::run_bulk_read_queryable(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));
    tokio::spawn(acquisition::run(
        session.clone(),
        handle.clone(),
        shutdown.clone(),
    ));

    // Periodic announce.
    let announce_session = session.clone();
    let announce_device_id = device_id.clone();
    let announce_name = device_info
        .model
        .clone()
        .unwrap_or_else(|| "U3V Camera".to_string());
    let announce_model = device_info
        .model
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());
    let announce_serial = device_info
        .serial
        .clone()
        .unwrap_or_else(|| "N/A".to_string());
    let mut announce_shutdown = shutdown;
    tokio::spawn(async move {
        let announce = DeviceAnnounce {
            id: announce_device_id.clone(),
            name: announce_name,
            model: announce_model,
            serial: announce_serial,
            api_version: Some(API_VERSION),
        };
        let key = keys::announce(&announce_device_id);
        let payload = serde_json::to_vec(&announce).unwrap();
        loop {
            let _ = announce_session.put(&key, payload.clone()).await;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                _ = announce_shutdown.changed() => {
                    if *announce_shutdown.borrow() { break; }
                }
            }
        }
    });

    info!(device_id, "U3V service tasks spawned");
    Ok(())
}

fn init_tracing(verbose: u8) {
    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
