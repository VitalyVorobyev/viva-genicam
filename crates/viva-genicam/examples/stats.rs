use std::env;
use std::error::Error;
use std::time::Duration;

use viva_genicam::gige::stats::StreamStatsAccumulator;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let drop_rate = env::args()
        .skip(1)
        .find_map(|arg| arg.strip_prefix("--drop-rate=").map(|v| v.to_string()))
        .unwrap_or_else(|| "0.0".into())
        .parse::<f32>()?;
    let stats = StreamStatsAccumulator::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    println!(
        "Simulated stream stats (drop rate {:.1}%)",
        drop_rate * 100.0
    );
    for _ in 0..5 {
        ticker.tick().await;
        // Update counters with synthetic values.
        stats.record_packet();
        if drop_rate > 0.0 {
            stats.record_backpressure_drop();
            stats.record_resend();
            stats.record_resend_ranges(1);
        }
        let snapshot = stats.snapshot();
        println!(
            "packets={:>6} drops={:>3} resends={:>3} backpressure={:>3}",
            snapshot.packets, snapshot.drops, snapshot.resends, snapshot.backpressure_drops
        );
    }
    Ok(())
}
