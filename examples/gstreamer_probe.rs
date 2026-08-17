//! End-to-end smoke test for the GStreamer local-capture streaming path.
//!
//! ```text
//! cargo run --features input-gstreamer --example gstreamer_probe
//! ```
//!
//! Enumerates devices via `query(ApiBackend::GStreamer)`, opens the
//! first one with `GStreamerCaptureDevice::new`, builds the
//! `source ! capsfilter ! videoconvert ! appsink` pipeline, pulls 5
//! frames, and prints a summary. Used for hardware verification in
//! WSL/usbipd setups, exercising the GStreamer backend directly without
//! going through `nokhwa::open()`.

#[cfg(feature = "input-gstreamer")]
fn main() -> Result<(), nokhwa_core::error::NokhwaError> {
    use nokhwa::backends::capture::GStreamerCaptureDevice;
    use nokhwa::query;
    use nokhwa_core::traits::FrameSource;
    use nokhwa_core::types::{
        color_frame_formats, ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType,
    };

    let cameras = query(ApiBackend::GStreamer)?;
    println!("Detected {} GStreamer source(s):", cameras.len());
    for (i, c) in cameras.iter().enumerate() {
        println!("  [{i}] {} | {}", c.human_name(), c.description());
    }
    if cameras.is_empty() {
        return Err(nokhwa_core::error::NokhwaError::general(
            "no GStreamer sources visible — is a webcam plugged in / attached via usbipd?",
        ));
    }

    // 640x480 NV12 30fps is the happy-path reference: small
    // enough to fit under WSL + usbip bandwidth budgets, big enough to
    // be meaningful, and exercises `videoconvert` when the device
    // prefers YUY2 natively. Higher resolutions work on direct USB
    // but may hit `usbipd` bandwidth caps in WSL.
    let req = RequestedFormat::with_formats(
        RequestedFormatType::Closest(nokhwa_core::types::CameraFormat::new(
            nokhwa_core::types::Resolution::new(640, 480),
            nokhwa_core::types::FrameFormat::NV12,
            30,
        )),
        color_frame_formats(),
    );
    let mut cam = GStreamerCaptureDevice::new(&CameraIndex::Index(0), req)?;
    println!("Negotiated: {:?}", cam.negotiated_format());

    cam.open()?;
    for i in 0..5 {
        let f = cam.frame()?;
        println!(
            "  frame[{i}]: {} bytes {:?} @ {}x{}",
            f.buffer().len(),
            f.source_frame_format(),
            f.resolution().width(),
            f.resolution().height()
        );
    }

    // Controls smoke: list live controls, round-trip brightness.
    // Graceful when the source element doesn't expose any (Windows
    // ksvideosrc, macOS avfvideosrc) — `controls()` returns an empty
    // list and the round-trip is skipped.
    {
        use nokhwa_core::traits::CameraDevice;
        use nokhwa_core::types::{ControlValueDescription, ControlValueSetter, KnownCameraControl};
        let ctrls = cam.controls()?;
        println!("Controls: {} live entries", ctrls.len());
        for c in &ctrls {
            println!("  {}: {:?}", c.control(), c.description());
        }
        if let Some(brightness) = ctrls
            .iter()
            .find(|c| c.control() == KnownCameraControl::Brightness)
        {
            if let ControlValueDescription::IntegerRange {
                min, max, value, ..
            } = brightness.description()
            {
                let target: i64 = if *value < *max {
                    *value + 1
                } else if *value > *min {
                    *value - 1
                } else {
                    *value
                };
                if target != *value {
                    cam.set_control(
                        KnownCameraControl::Brightness,
                        ControlValueSetter::Integer(target),
                    )?;
                    let after = cam.controls()?;
                    let updated = after
                        .iter()
                        .find(|c| c.control() == KnownCameraControl::Brightness)
                        .expect("brightness control disappeared after set");
                    match updated.description() {
                        ControlValueDescription::IntegerRange { value: v2, .. } => {
                            assert_eq!(
                                *v2, target,
                                "Brightness did not round-trip: wanted {target}, got {v2}"
                            );
                            println!("Brightness round-trip OK: {value} -> {target} (verified)");
                        },
                        d => panic!("Brightness description variant changed: {d:?}"),
                    }
                    // Restore original to be polite to subsequent runs.
                    let _ = cam.set_control(
                        KnownCameraControl::Brightness,
                        ControlValueSetter::Integer(*value),
                    );
                }
            }
        }
    }

    cam.close()?;
    println!("Done.");
    Ok(())
}

#[cfg(not(feature = "input-gstreamer"))]
fn main() {
    eprintln!(
        "This example requires the `input-gstreamer` feature.\n\
         Try: cargo run --features input-gstreamer --example gstreamer_probe"
    );
}
