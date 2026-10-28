use super::*;
use futures_util::StreamExt;

async fn enable_timestamp(backend: &EmbeddedBackend) {
    backend
        .set_feature("ChunkModeActive", &serde_json::json!(true))
        .await
        .unwrap();
    backend
        .set_feature("ChunkSelector", &serde_json::json!("Timestamp"))
        .await
        .unwrap();
    backend
        .set_feature("ChunkEnable", &serde_json::json!(true))
        .await
        .unwrap();
}

fn assert_timestamp(metadata: Option<viva_zenoh_api::FrameMetadata>) {
    let metadata = metadata.expect("same-frame metadata");
    assert!(metadata.complete);
    assert!(!metadata.chunks.is_empty());
    let timestamp = metadata
        .features
        .iter()
        .find(|feature| feature.name == "ChunkTimestamp")
        .expect("XML decoded timestamp");
    assert_eq!(timestamp.value, metadata.ts_dev);
    assert!(timestamp.error.is_none());
    assert!(
        metadata.ts_host_ns.is_none(),
        "an uncalibrated clock is not host reception time"
    );
    assert!(metadata.block_id.parse::<u64>().is_ok());
}

#[test]
fn gige_preview_carries_this_frames_chunk_values() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let backend = EmbeddedBackend::new(Default::default());
        backend.connect("127.0.0.1").await.unwrap();
        enable_timestamp(&backend).await;
        let stream = backend.start_acquisition(Default::default()).await.unwrap();
        let (metadata, bmp) = fake_camera::first_preview(&stream.ws_url).await;
        assert!(bmp.starts_with(b"BM"));
        assert_timestamp(metadata);
        backend.shutdown().await;
        fake.stop().await;
    });
}

#[test]
fn usb_preview_carries_this_frames_chunk_values() {
    tauri::async_runtime::block_on(async {
        let device = viva_fake_u3v::FakeU3vCamera::new(64, 48)
            .open_device()
            .unwrap();
        let (camera, _) = viva_genicam::open_u3v_device(device).unwrap();
        let backend = EmbeddedBackend::new(Default::default());
        backend
            .attach(Box::new(camera), "usb-metadata-test", None)
            .await;
        enable_timestamp(&backend).await;
        let stream = backend.start_acquisition(Default::default()).await.unwrap();
        let (metadata, _) = fake_camera::first_preview(&stream.ws_url).await;
        assert_timestamp(metadata);
        backend.shutdown().await;
    });
}

#[test]
fn plain_frame_metadata_does_not_wait_for_the_camera_mutex() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let backend = EmbeddedBackend::new(Default::default());
        backend.connect("127.0.0.1").await.unwrap();
        let stream = backend.start_acquisition(Default::default()).await.unwrap();
        let (mut socket, _) = tokio_tungstenite::connect_async(&stream.ws_url)
            .await
            .unwrap();
        let slot = backend.camera.lock().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut previous = None;
            loop {
                if let Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes))) =
                    socket.next().await
                {
                    let (metadata, _) = viva_streamer::preview::decode(&bytes).unwrap();
                    let metadata = metadata.unwrap();
                    assert!(metadata.chunks.is_empty());
                    assert!(metadata.features.is_empty());
                    if previous.as_ref().is_some_and(|id| id != &metadata.block_id) {
                        break;
                    }
                    previous = Some(metadata.block_id);
                }
            }
        })
        .await
        .expect("plain frames keep arriving while the camera mutex is occupied");
        drop(slot);
        drop(socket);
        backend.shutdown().await;
        fake.stop().await;
    });
}
