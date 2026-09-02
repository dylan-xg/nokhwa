//! Non-device integration tests for the root nokhwa crate.
//! These verify that public API re-exports are accessible from outside the crate.

use nokhwa::utils::*;
use nokhwa::NokhwaError;

#[test]
fn requested_format_creation_with_formats() {
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestResolution,
        &[FrameFormat::MJPEG, FrameFormat::YUYV],
    );
    assert_eq!(
        req.requested_format_type(),
        RequestedFormatType::AbsoluteHighestResolution
    );
}

#[test]
fn camera_index_from_u32() {
    let idx = CameraIndex::Index(3);
    assert!(idx.is_index());
    assert_eq!(idx.as_index().unwrap(), 3);
    assert!(!idx.is_string());
}

#[test]
fn camera_index_from_string() {
    let idx = CameraIndex::String("/dev/video0".to_string());
    assert!(idx.is_string());
    assert_eq!(idx.as_string(), "/dev/video0");
    assert!(!idx.is_index());
    // `as_index` (`nokhwa-core/src/types.rs:315-322`) wraps `ParseIntError`
    // through `NokhwaError::general` → `GeneralError { backend: None, .. }`.
    // Pin the variant + libstd's `ParseIntError` Display string so a
    // regression that reroutes the helper or attaches a backend tag is
    // caught at the public API surface (this is a non-device integration
    // test that re-exports the type, so the pin doubles as a re-export
    // sanity check).
    let err = idx.as_index().expect_err("non-numeric String should err");
    match err {
        NokhwaError::GeneralError { message, backend } => {
            assert_eq!(message, "invalid digit found in string");
            assert!(
                backend.is_none(),
                "expected no backend tag, got {backend:?}"
            );
        },
        other => panic!("expected GeneralError, got {other:?}"),
    }
}

#[test]
fn camera_index_default() {
    let idx = CameraIndex::default();
    assert_eq!(idx, CameraIndex::Index(0));
}

#[test]
fn camera_format_default_values() {
    let fmt = CameraFormat::default();
    assert_eq!(fmt.resolution(), Resolution::new(640, 480));
    assert_eq!(fmt.format(), FrameFormat::MJPEG);
    assert_eq!(fmt.frame_rate(), 30);
}

#[test]
fn resolution_display_exact_format() {
    // `Resolution`'s `Display` impl at `nokhwa-core/src/types.rs:571-575`
    // writes `"{w}x{h}"` — pin the exact format. The previous
    // contains-only test would still pass if the separator changed
    // from `x` to `×`, `*`, or whitespace, or if the width / height
    // ordering flipped — all of which would silently break downstream
    // parsers and any logged-resolution assertion in user code.
    let res = Resolution::new(1920, 1080);
    assert_eq!(format!("{res}"), "1920x1080");
}

#[test]
fn camera_info_construction() {
    let info = CameraInfo::new(
        "Test Camera".to_string(),
        "A test camera".to_string(),
        "misc".to_string(),
        false,
        CameraIndex::Index(0),
    );
    assert_eq!(info.human_name(), "Test Camera");
    assert_eq!(info.description(), "A test camera");
    assert_eq!(info.misc(), "misc");
    assert_eq!(info.index(), &CameraIndex::Index(0));
}
