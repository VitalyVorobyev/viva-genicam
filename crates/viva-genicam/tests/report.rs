//! The diagnostic report, written against the in-process fake camera.
//!
//! `viva-camctl report`, `viva-service` and Viva Studio all produce it with
//! `viva_genicam::report::build_report`, so this pins what each of them hands
//! a reporter (backlog ST-41).

use std::net::Ipv4Addr;
use std::time::Duration;

use viva_fake_gige::FakeCamera;
use viva_genicam::report::{ReportRequest, ReportTarget, build_report};

async fn fake() -> FakeCamera {
    loop {
        match FakeCamera::builder()
            .bind_ip([127, 0, 0, 1].into())
            .port(3956)
            .build()
            .await
        {
            Ok(camera) => return camera,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("failed to start the fake camera: {e}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_report_on_a_camera_carries_its_registers_xml_and_features() {
    let _camera = fake().await;
    let mut request = ReportRequest::new("report-test 1.0", ReportTarget::Ip(Ipv4Addr::LOCALHOST));
    request.notes.push("mode:        test".to_string());
    let report = build_report(&request).await;

    for expected in [
        "tool:        report-test 1.0",
        "mode:        test",
        "## Network interfaces",
        "selected: 127.0.0.1",
        "GevHeartbeatTimeout",
        "No features were dropped.",
        "<RegisterDescription",
        "## End of report",
    ] {
        assert!(report.contains(expected), "missing {expected:?}:\n{report}");
    }

    // Without the XML, the feature summary stays and the document goes.
    request.include_xml = false;
    let short = build_report(&request).await;
    assert!(short.contains("No features were dropped."));
    assert!(!short.contains("<RegisterDescription"));
}

/// A camera nobody answers for still gets a report saying so, not an error.
#[tokio::test]
async fn a_report_on_an_absent_camera_says_why() {
    let request = ReportRequest {
        discovery_timeout: Duration::from_millis(100),
        ..ReportRequest::new("report-test 1.0", ReportTarget::Index(7))
    };
    let report = build_report(&request).await;
    assert!(report.contains("No camera selected"), "{report}");
    assert!(report.contains("## End of report"));
}

/// A camera the caller holds open is described from what the caller read. A
/// USB3 Vision device admits one owner, so the report cannot open it a second
/// time, and `viva-camctl report` had nothing to offer a U3V reporter
/// (backlog DX-06).
#[cfg(feature = "u3v")]
#[tokio::test]
async fn report_describes_a_camera_the_caller_holds_open() {
    use viva_genicam::report::OpenCamera;

    let device = viva_fake_u3v::FakeU3vCamera::new(64, 48)
        .open_device()
        .expect("fake U3V camera");
    let (_camera, xml) = viva_genicam::open_u3v_device(device).expect("open");
    let camera = OpenCamera {
        transport: "USB3 Vision".to_string(),
        identity: vec![("model".to_string(), "FakeU3V".to_string())],
        xml,
    };
    let request = ReportRequest {
        discovery_timeout: Duration::from_millis(50),
        ..ReportRequest::new("test", ReportTarget::Open(camera))
    };

    let report = build_report(&request).await;

    assert!(report.contains("transport: USB3 Vision"), "{report}");
    assert!(report.contains("model: FakeU3V"), "{report}");
    assert!(
        report.contains("built:"),
        "the nodemap was not built: {report}"
    );
    assert!(report.contains("<RegisterDescription"), "no XML: {report}");
}
