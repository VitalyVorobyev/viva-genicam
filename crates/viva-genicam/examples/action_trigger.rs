use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};

use viva_genicam::gige::action::{send_action, ActionParams};
use viva_genicam::gige::GVCP_PORT;

fn parse_u32_arg(value: &str) -> Result<u32, Box<dyn Error>> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Ok(u32::from_str_radix(hex, 16)?)
    } else {
        Ok(trimmed.parse()?)
    }
}

fn parse_u64_arg(value: &str) -> Result<u64, Box<dyn Error>> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Ok(u64::from_str_radix(hex, 16)?)
    } else {
        Ok(trimmed.parse()?)
    }
}

#[derive(Debug, Clone)]
struct Args {
    target: IpAddr,
    device_key: u32,
    group_key: u32,
    group_mask: u32,
    channel: u16,
    schedule: Option<u64>,
    timeout_ms: u64,
}

fn print_usage() {
    eprintln!("usage: action_trigger --broadcast <ipv4> [options]\n");
    eprintln!("Options:");
    eprintln!("  --device-key <u32>     Device key (default: 0)");
    eprintln!("  --group-key <u32>      Group key (default: 0)");
    eprintln!("  --group-mask <u32>     Group mask (default: 0xFFFF_FFFF)");
    eprintln!("  --channel <u16>        Stream channel selector (default: 0)");
    eprintln!("  --schedule <ticks>     Optional scheduled time in device ticks");
    eprintln!("  --timeout-ms <u64>     Wait time for acknowledgements (default: 200)");
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut target = None;
    let mut device_key = 0u32;
    let mut group_key = 0u32;
    let mut group_mask = 0xFFFF_FFFFu32;
    let mut channel = 0u16;
    let mut schedule = None;
    let mut timeout_ms = 200u64;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--broadcast" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--broadcast requires an IP address".to_string())?;
                target = Some(value.parse()?);
            }
            "--device-key" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--device-key requires a value".to_string())?;
                device_key = parse_u32_arg(&value)?;
            }
            "--group-key" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--group-key requires a value".to_string())?;
                group_key = parse_u32_arg(&value)?;
            }
            "--group-mask" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--group-mask requires a value".to_string())?;
                group_mask = parse_u32_arg(&value)?;
            }
            "--channel" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--channel requires a value".to_string())?;
                channel = value.parse()?;
            }
            "--schedule" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--schedule requires ticks".to_string())?;
                schedule = Some(parse_u64_arg(&value)?);
            }
            "--timeout-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--timeout-ms requires a value".to_string())?;
                timeout_ms = value.parse()?;
            }
            "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let target = target.ok_or_else(|| "--broadcast is required".to_string())?;
    Ok(Args {
        target,
        device_key,
        group_key,
        group_mask,
        channel,
        schedule,
        timeout_ms,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt::init();
    let args = parse_args()?;
    let destination = SocketAddr::new(args.target, GVCP_PORT);
    println!("Dispatching action command to {destination}");
    let params = ActionParams {
        device_key: args.device_key,
        group_key: args.group_key,
        group_mask: args.group_mask,
        scheduled_time: args.schedule,
        channel: args.channel,
    };
    let summary = send_action(destination, &params, args.timeout_ms).await?;
    println!(
        "Sent {} datagram, received {} acknowledgements (timeout: {} ms)",
        summary.sent, summary.acks, args.timeout_ms
    );
    println!("Done");
    Ok(())
}
