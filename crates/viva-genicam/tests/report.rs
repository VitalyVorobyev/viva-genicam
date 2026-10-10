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
