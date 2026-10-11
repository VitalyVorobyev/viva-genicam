use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use viva_genapi_xml::{self, XmlError};
use viva_genicam::Camera;
use viva_genicam::genapi;
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::gige::nic::IfaceSelector;

#[derive(Debug, Clone)]
struct Args {
    iface: IfaceSelector,
    port: u16,
    enable: Vec<String>,
    limit: usize,
}

fn print_usage() {
    eprintln!(
        "usage: events_gige --iface <HOST-IP|NAME> [--port <udp-port>] [--enable <event1,event2>] [--count <n>]"
    );
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut iface = None;
    let mut port = 10020u16;
    let mut enable = Vec::new();
    let mut limit = 10usize;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iface" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--iface requires a host IP or interface name".to_string())?;
                // Either spelling, as everywhere else (#109).
                iface = Some(value.parse::<IfaceSelector>().unwrap());
            }
            "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--port requires a value".to_string())?;
                port = value.parse()?;
            }
            "--enable" => {
                let list = args
                    .next()
                    .ok_or_else(|| "--enable requires a comma separated list".to_string())?;
                enable = list
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
            }
            "--count" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--count requires an integer".to_string())?;
                limit = value.parse()?;
            }
            "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

    let iface = iface.ok_or_else(|| "--iface is required".to_string())?;
    if enable.is_empty() {
        enable = vec!["FrameStart".into(), "ExposureEnd".into()];
    }

    Ok(Args {
        iface,
        port,
        enable,
        limit,
    })
}

async fn build_camera(
    control: Arc<Mutex<viva_genicam::gige::GigeDevice>>,
) -> Result<Camera<viva_genicam::GigeRegisterIo>, Box<dyn Error>> {
    let xml = viva_genapi_xml::fetch_and_load_xml({
        let control = Arc::clone(&control);
        move |addr, len| {
            let control = Arc::clone(&control);
            async move {
                let mut dev = control.lock().await;
                dev.read_mem(addr, len)
                    .await
                    .map_err(|err| XmlError::Transport(err.to_string()))
            }
        }
    })
    .await?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = genapi::NodeMap::try_from_xml(model)?;
    let handle = tokio::runtime::Handle::current();
    let device = Arc::try_unwrap(control)
        .map_err(|_| "control connection still in use")?
        .into_inner();
    let transport = viva_genicam::GigeRegisterIo::new(handle, device);
    Ok(Camera::new(transport, nodemap))
}

fn format_event(event: &viva_genicam::Event, index: usize) {
    println!(
        "#{index:02} | host={:?} | id=0x{:04X} | ticks={} | payload={} bytes",
        event.ts_host,
        event.id,
        event.ts_dev,
        event.data.len()
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    // The camera sends events *to* this address, so it must be the host's.
    // `--iface` names the interface either way (#109); the address comes from
    // resolving it, not from parsing the argument.
    let iface = args.iface.resolve()?;
    let local_ip = iface
        .ipv4()
        .ok_or_else(|| format!("interface {} has no IPv4 address", iface.name()))?;
    println!(
        "Configuring message channel on {}:{} (events: {})",
        local_ip,
        args.port,
        args.enable.join(", ")
    );

    let timeout = Duration::from_millis(500);
    let mut devices = viva_genicam::gige::discover(timeout).await?;
    let device = devices
        .pop()
        .ok_or_else(|| "no GigE Vision cameras discovered".to_string())?;

    let control_addr = SocketAddr::new(std::net::IpAddr::V4(device.ip), GVCP_PORT);
    let control = Arc::new(Mutex::new({
        let mut device = viva_genicam::gige::GigeDevice::open(control_addr).await?;
        // The adapter built from it keeps control alive, and releases it
        // when dropped.
        device.claim_control().await?;
        device
    }));
    let mut camera = build_camera(control).await?;

    let enable_refs: Vec<&str> = args.enable.iter().map(|s| s.as_str()).collect();
    camera
        .configure_events(local_ip, args.port, &enable_refs)
        .await?;
    let stream = camera.open_event_stream(local_ip, args.port).await?;
    if let Ok(addr) = stream.local_addr() {
        println!("Listening on {addr}");
    }

    for idx in 0..args.limit {
        match stream.next().await {
            Ok(event) => format_event(&event, idx + 1),
            Err(err) => {
                eprintln!("Failed to receive event: {err}");
                break;
            }
        }
    }

    println!("Event streaming demo complete");
    Ok(())
}
