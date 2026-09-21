//! Raw frame recording engine.
//!
//! Records incoming frames to a `.gsr` (GenICam Studio Recording) file format:
//!
//! ```text
//! [JSON header (null-terminated)]
//! [Frame 0: 16-byte FrameHeader + raw pixel data]
//! [Frame 1: 16-byte FrameHeader + raw pixel data]
//! ...
//! [Frame index JSON (appended on stop)]
//! ```

use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc, watch};

use viva_zenoh_api::{FrameHeader, HEADER_SIZE};

/// JSON header written at the start of a .gsr file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GsrHeader {
    pub version: u32,
    pub pixel_format: String,
    pub width: u32,
    pub height: u32,
    pub device_id: String,
    pub started_at: String, // ISO 8601
}

/// Per-frame index entry, appended as JSON array at end of file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameIndexEntry {
    pub seq: u32,
    pub offset: u64,
    pub size: u64,
    pub elapsed_ms: u64,
}

/// Recording status exposed to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingStatus {
    pub active: bool,
    pub file_path: Option<String>,
    pub frame_count: u64,
    pub bytes_written: u64,
    pub elapsed_secs: f64,
}

impl Default for RecordingStatus {
    fn default() -> Self {
        Self {
            active: false,
            file_path: None,
            frame_count: 0,
            bytes_written: 0,
            elapsed_secs: 0.0,
        }
    }
}

/// Frame data sent through the recording channel.
pub struct RecordFrame {
    pub header_bytes: Vec<u8>,
    pub pixel_data: Vec<u8>,
}

/// Handle to a running recording session.
pub struct RecordingHandle {
    pub tx: mpsc::Sender<RecordFrame>,
    pub status: Arc<Mutex<RecordingStatus>>,
    pub shutdown_tx: watch::Sender<bool>,
}

/// Start a background recording task that writes frames to a .gsr file.
///
/// Returns a handle with a channel sender for feeding frames and a status monitor.
pub fn start_recording(
    output_path: PathBuf,
    gsr_header: GsrHeader,
) -> Result<RecordingHandle, io::Error> {
    let (tx, rx) = mpsc::channel::<RecordFrame>(64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status = Arc::new(Mutex::new(RecordingStatus {
        active: true,
        file_path: Some(output_path.display().to_string()),
        frame_count: 0,
        bytes_written: 0,
        elapsed_secs: 0.0,
    }));

    let status_clone = status.clone();
    let path = output_path.clone();

    tokio::spawn(async move {
        if let Err(e) = recording_writer_task(path, gsr_header, rx, shutdown_rx, status_clone).await
        {
            tracing::error!("Recording writer error: {e}");
        }
    });

    Ok(RecordingHandle {
        tx,
        status,
        shutdown_tx,
    })
}

async fn recording_writer_task(
    path: PathBuf,
    header: GsrHeader,
    mut rx: mpsc::Receiver<RecordFrame>,
    mut shutdown: watch::Receiver<bool>,
    status: Arc<Mutex<RecordingStatus>>,
) -> io::Result<()> {
    let file = std::fs::File::create(&path)?;
    let mut writer = BufWriter::new(file);

    // Write JSON header (null-terminated)
    let header_json = serde_json::to_vec(&header).unwrap_or_default();
    writer.write_all(&header_json)?;
    writer.write_all(&[0u8])?; // null terminator

    let data_start = header_json.len() as u64 + 1;
    let mut index: Vec<FrameIndexEntry> = Vec::new();
    let mut current_offset = data_start;
    let mut bytes_written = data_start;
    let start_time = Instant::now();

    loop {
        tokio::select! {
            frame = rx.recv() => {
                let frame = match frame {
                    Some(f) => f,
                    None => break, // channel closed
                };

                let frame_size = (frame.header_bytes.len() + frame.pixel_data.len()) as u64;

                // Parse seq from header bytes
                let seq = if frame.header_bytes.len() >= HEADER_SIZE {
                    FrameHeader::decode(&frame.header_bytes)
                        .map(|(h, _)| h.seq)
                        .unwrap_or(0)
                } else {
                    0
                };

                // Write frame header + pixel data
                writer.write_all(&frame.header_bytes)?;
                writer.write_all(&frame.pixel_data)?;

                let elapsed_ms = start_time.elapsed().as_millis() as u64;
                index.push(FrameIndexEntry {
                    seq,
                    offset: current_offset,
                    size: frame_size,
                    elapsed_ms,
                });

                current_offset += frame_size;
                bytes_written += frame_size;

                // Update status
                let mut s = status.lock().await;
                s.frame_count = index.len() as u64;
                s.bytes_written = bytes_written;
                s.elapsed_secs = start_time.elapsed().as_secs_f64();
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }
    }

    // Flush remaining data
    writer.flush()?;

    // Append frame index as JSON at end of file
    let index_json = serde_json::to_vec_pretty(&index).unwrap_or_default();
    writer.write_all(&index_json)?;
    writer.flush()?;

    // Mark recording complete
    let mut s = status.lock().await;
    s.active = false;
    s.elapsed_secs = start_time.elapsed().as_secs_f64();

    tracing::info!(
        "Recording complete: {} frames, {:.1} MB, {:.1}s → {}",
        index.len(),
        bytes_written as f64 / 1_048_576.0,
        s.elapsed_secs,
        path.display()
    );

    Ok(())
}

/// Generate a timestamped output path for a new recording.
pub fn default_recording_path(base_dir: &Path, device_id: &str) -> PathBuf {
    let timestamp = chrono_lite_timestamp();
    let sanitized = device_id.replace(['/', '\\', ':'], "_");
    base_dir.join(format!("{sanitized}_{timestamp}.gsr"))
}

fn chrono_lite_timestamp() -> String {
    // Simple timestamp without chrono dependency
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gsr_header_serializes() {
        let header = GsrHeader {
            version: 1,
            pixel_format: "Mono8".to_string(),
            width: 640,
            height: 480,
            device_id: "cam0".to_string(),
            started_at: "2026-04-04T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&header).unwrap();
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"pixel_format\":\"Mono8\""));
    }

    #[test]
    fn test_default_recording_path() {
        let path = default_recording_path(Path::new("/tmp"), "cam/0");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("cam_0_"));
        assert!(name.ends_with(".gsr"));
    }

    #[test]
    fn test_recording_status_default() {
        let status = RecordingStatus::default();
        assert!(!status.active);
        assert_eq!(status.frame_count, 0);
    }
}
