use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::info;
use viva_genicam::genapi::{NodeMap, RegisterIo};
use viva_genicam::gige::GVCP_PORT;
use viva_genicam::{self, sfnc, Camera, GenicamError, GigeRegisterIo};

#[derive(Debug)]
struct Args {
    samples: usize,
    interval_ms: u64,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            samples: 16,
            interval_ms: 50,
        }
    }
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--samples requires a value".to_string())?;
                parsed.samples = value.parse()?;
            }
            "--interval-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--interval-ms requires a value".to_string())?;
                parsed.interval_ms = value.parse()?;
            }
            "--help" => {
                println!(
                    "usage: time_sync [--samples N] [--interval-ms M]\n\
                     defaults: N=16, M=50"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(parsed)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;

    let timeout = Duration::from_millis(500);
    let mut devices = viva_genicam::gige::discover(timeout).await?;
    if devices.is_empty() {
        println!("No devices discovered for time synchronisation demo");
        return Ok(());
    }
    let device = devices.remove(0);
    println!(
        "Using camera {}",
        device
            .model
            .clone()
            .unwrap_or_else(|| "<unknown>".to_string())
    );

    let control_addr = SocketAddr::new(IpAddr::V4(device.ip), GVCP_PORT);
    let control = Arc::new(Mutex::new(
        viva_genicam::gige::GigeDevice::open(control_addr).await?,
    ));
    let xml = viva_genapi_xml::fetch_and_load_xml({
        let control = control.clone();
        move |address, length| {
            let control = control.clone();
            async move {
                let mut dev = control.lock().await;
                dev.read_mem(address, length)
                    .await
                    .map_err(|err| viva_genapi_xml::XmlError::Transport(err.to_string()))
            }
        }
    })
    .await?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::from(model);
    let handle = tokio::runtime::Handle::current();
    let control_device = match Arc::try_unwrap(control) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => return Err("control connection still in use".into()),
    };
    let transport = GigeRegisterIo::new(handle, control_device);
    let mut camera = Camera::new(transport, nodemap);

    // Ensure the device timestamp counter starts from a consistent state.
    let _ = camera.time_reset();
    camera
        .time_calibrate(args.samples, args.interval_ms)
        .await?;
    let sync = camera.time_sync();
    if let Some(freq) = sync.freq_hz() {
        println!("Reported device frequency: {freq:.3} Hz");
    } else {
        println!("Device frequency not reported; using fitted model only");
    }
    let (a, b) = sync.coefficients();
    println!("Fitted host_time = {a:.9} * ticks + {b:.6}");

    let latch_cmd = find_alias(camera.nodemap(), sfnc::TS_LATCH_CMDS)
        .ok_or("timestamp latch command not available")?;
    let value_node = find_alias(camera.nodemap(), sfnc::TS_VALUE_NODES)
        .ok_or("timestamp value node not available")?;

    println!("Mapping the next 5 device timestamps:");
    for idx in 0..5 {
        execute_command(&mut camera, latch_cmd)?;
        let ticks = read_timestamp(&camera, value_node)?;
        let mapped = camera.map_dev_ts(ticks);
        println!("  sample #{idx}: {ticks} -> {:?}", mapped);
        if args.interval_ms > 0 {
            tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
        }
    }

    info!("Time synchronisation demo complete");
    Ok(())
}

fn find_alias(nodemap: &NodeMap, names: &[&'static str]) -> Option<&'static str> {
    names
        .iter()
        .copied()
        .find(|name| nodemap.node(name).is_some())
}

fn execute_command<T: RegisterIo>(
    camera: &mut Camera<T>,
    command: &str,
) -> Result<(), GenicamError> {
    let transport_ptr = camera.transport() as *const T;
    unsafe {
        camera
            .nodemap_mut()
            .exec_command(command, &*transport_ptr)
            .map_err(GenicamError::from)
    }
}

fn read_timestamp<T: RegisterIo>(camera: &Camera<T>, node: &str) -> Result<u64, GenicamError> {
    let value = camera
        .nodemap()
        .get_integer(node, camera.transport())
        .map_err(GenicamError::from)?;
    u64::try_from(value)
        .map_err(|_| GenicamError::Transport("timestamp node returned negative value".into()))
}
