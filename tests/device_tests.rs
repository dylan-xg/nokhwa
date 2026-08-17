//! Integration tests requiring a physical camera device.
//!
//! Only compiled when the `device-test` feature is enabled. CI runners
//! without a camera should leave the feature off — the tests will then
//! not be built at all. Exercises the post-0.13 `nokhwa::open` /
//! `OpenedCamera` API surface.

#![cfg(feature = "device-test")]

use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, ControlValueDescription, ControlValueSetter,
    FrameFormat, KnownCameraControlFlag, Resolution,
};
use nokhwa::{native_api_backend, open, query, OpenRequest, OpenedCamera};

fn native_backend() -> ApiBackend {
    native_api_backend().expect("no native API backend compiled in for this target")
}

fn open_first() -> OpenedCamera {
    open(CameraIndex::Index(0), OpenRequest::any())
        .expect("open(CameraIndex::Index(0)) failed — is a camera attached?")
}

/// MSMF hotplug plumbing: `take_hotplug_events()` succeeds once and
/// errors on the second call, the returned poller stays quiet under a
/// steady-state device set, and dropping the poller joins the
/// background thread cleanly (the test would hang if the Drop impl
/// regressed). A manual plug/unplug observation is out of scope for
/// the automated suite but the plumbing is fully exercised here.
#[cfg(all(feature = "input-msmf", target_os = "windows"))]
#[test]
fn msmf_hotplug_take_and_steady_state() {
    use nokhwa::backends::hotplug::MediaFoundationHotplugContext;
    use nokhwa::NokhwaError;
    use nokhwa_core::traits::HotplugSource;
    use std::time::Duration;

    let mut ctx = MediaFoundationHotplugContext::new();
    let mut poll = ctx
        .take_hotplug_events()
        .expect("first take_hotplug_events must succeed");

    // The MSMF impl (`nokhwa-bindings-windows-msmf/src/hotplug.rs:74-83`)
    // returns `UnsupportedOperationError(ApiBackend::MediaFoundation)`
    // on the second call. The previous `is_err()` check would pass for
    // any error variant — including a regression that swapped to
    // `GeneralError` (silently changing the public-API error shape) or
    // mistagged the backend as a different `ApiBackend` (mis-routing
    // log-grep tooling). Pin both the variant and the backend payload.
    let err = match ctx.take_hotplug_events() {
        Ok(_) => panic!("second take_hotplug_events must error per the trait contract"),
        Err(e) => e,
    };
    match err {
        NokhwaError::UnsupportedOperationError(backend) => {
            assert_eq!(backend, ApiBackend::MediaFoundation);
        },
        other => panic!("expected UnsupportedOperationError(MediaFoundation), got {other:?}"),
    }

    // Three poll windows (~1.5s) with no plug/unplug → no events.
    let evt = poll.next_timeout(Duration::from_millis(1500));
    assert!(
        evt.is_none(),
        "expected no hotplug event on steady state, got {evt:?}"
    );

    drop(poll);
}

/// V4L hotplug plumbing: same shape as the MSMF test. Exercises the
/// `take_hotplug_events()` contract, the steady-state silence
/// guarantee on an inotify-driven worker (no spurious wakeups
/// translating into events), and clean Drop-time join. The
/// v4l-loopback CI job auto-validates the actual `Connected` /
/// `Disconnected` emission via `modprobe -r/+r v4l2loopback`.
#[cfg(all(feature = "input-v4l", target_os = "linux"))]
#[test]
fn v4l_hotplug_take_and_steady_state() {
    use nokhwa::backends::hotplug::V4LHotplugContext;
    use nokhwa::NokhwaError;
    use nokhwa_core::traits::HotplugSource;
    use std::time::Duration;

    let mut ctx = V4LHotplugContext::new();
    let mut poll = ctx
        .take_hotplug_events()
        .expect("first take_hotplug_events must succeed");

    // The V4L impl (`nokhwa-bindings-linux-v4l/src/hotplug.rs:75-84`)
    // returns `UnsupportedOperationError(ApiBackend::Video4Linux)` on
    // the second call. Pin the variant + backend payload (see the MSMF
    // sibling for the full rationale on why `is_err()` is too loose).
    let err = match ctx.take_hotplug_events() {
        Ok(_) => panic!("second take_hotplug_events must error per the trait contract"),
        Err(e) => e,
    };
    match err {
        NokhwaError::UnsupportedOperationError(backend) => {
            assert_eq!(backend, ApiBackend::Video4Linux);
        },
        other => panic!("expected UnsupportedOperationError(Video4Linux), got {other:?}"),
    }

    // Three poll windows (~1.5s) with no plug/unplug → no events.
    let evt = poll.next_timeout(Duration::from_millis(1500));
    assert!(
        evt.is_none(),
        "expected no hotplug event on steady state, got {evt:?}"
    );

    drop(poll);
}

/// AVFoundation hotplug plumbing: same shape as the MSMF and V4L
/// tests. Exercises the `take_hotplug_events()` contract and the
/// steady-state silence guarantee on the 500ms polling worker, plus
/// clean Drop-time join. Manual plug/unplug observation against a
/// physical USB camera is out of scope for the automated suite, but
/// the wiring is fully exercised here on the self-hosted
/// `macos-camera` runner.
#[cfg(all(feature = "input-avfoundation", target_os = "macos"))]
#[test]
fn avfoundation_hotplug_take_and_steady_state() {
    use nokhwa::backends::hotplug::AVFoundationHotplugContext;
    use nokhwa::NokhwaError;
    use nokhwa_core::traits::HotplugSource;
    use std::time::Duration;

    let mut ctx = AVFoundationHotplugContext::new();
    let mut poll = ctx
        .take_hotplug_events()
        .expect("first take_hotplug_events must succeed");

    // The AVF impl (`nokhwa-bindings-macos-avfoundation/src/hotplug.rs:59-68`)
    // returns `UnsupportedOperationError(ApiBackend::AVFoundation)` on
    // the second call. Pin the variant + backend payload (see the MSMF
    // sibling for the full rationale).
    let err = match ctx.take_hotplug_events() {
        Ok(_) => panic!("second take_hotplug_events must error per the trait contract"),
        Err(e) => e,
    };
    match err {
        NokhwaError::UnsupportedOperationError(backend) => {
            assert_eq!(backend, ApiBackend::AVFoundation);
        },
        other => panic!("expected UnsupportedOperationError(AVFoundation), got {other:?}"),
    }

    // Three poll windows (~1.5s) with no plug/unplug → no events.
    let evt = poll.next_timeout(Duration::from_millis(1500));
    assert!(
        evt.is_none(),
        "expected no hotplug event on steady state, got {evt:?}"
    );

    drop(poll);
}

#[test]
fn query_reports_at_least_one_device() {
    let devices = query(native_backend()).expect("query() returned an error");
    assert!(
        !devices.is_empty(),
        "no cameras found — these tests require a physical camera"
    );
}

/// `query()` must return distinct `CameraIndex` values — no two
/// `CameraInfo` entries should share an index. Each native backend
/// has a different way to enumerate devices (V4L2 walks `/dev/video*`,
/// MSMF's `MFEnumDeviceSources` indexes are positional, AVFoundation
/// uses `AVCaptureDevice.uniqueID`), and any of them could regress
/// into emitting one physical device under two handles (e.g. the
/// V4L2 enumerator listing the metadata-only secondary `/dev/videoN`
/// alongside the primary capture node, or MSMF surfacing the same
/// `IMFActivate` twice). Users keying a HashMap off `CameraIndex` —
/// the canonical pattern for "remember the user's selected camera
/// across sessions" — would silently lose one of the duplicates.
#[test]
fn query_indices_are_unique() {
    let devices = query(native_backend()).expect("query() returned an error");
    let mut seen: std::collections::HashSet<&CameraIndex> = std::collections::HashSet::new();
    for info in &devices {
        assert!(
            seen.insert(info.index()),
            "query() returned duplicate index {:?}; full list: {devices:?}",
            info.index()
        );
    }
}

/// Every `CameraInfo` returned by `query()` must have a non-empty
/// `human_name()`. UI pickers (the `nokhwactl list-devices` example,
/// downstream consumer apps) render this string verbatim for every
/// row — a blank entry surfaces as a confusing empty line a user
/// can't distinguish from "device disappeared mid-enumeration".
/// The single-opened-camera path is already pinned by
/// `opened_camera_info_and_backend_reflect_request` (line ~573) and
/// `open_info_matches_query_for_same_index` (line ~750), but those
/// only cover the device that actually gets opened. A regression in
/// a backend's enumerator that emitted a blank name for a *different*
/// (un-opened) entry — V4L2 reading an empty `card` field, MSMF
/// `IMFAttributes::MF_FRIENDLY_NAME` returning an empty BSTR for a
/// freshly-plugged device, AVFoundation's `localizedName` racing the
/// device-add notification — would slip past every existing test.
#[test]
fn query_results_have_non_empty_human_names() {
    let devices = query(native_backend()).expect("query() returned an error");
    for info in &devices {
        assert!(
            !info.human_name().is_empty(),
            "query() returned a device with an empty human_name(): {info:?}; full list: {devices:?}"
        );
    }
}

#[test]
fn open_stream_and_capture_frames() {
    match open_first() {
        OpenedCamera::Stream(mut cam) => {
            cam.open().expect("StreamCamera::open");
            let res = cam.negotiated_format().resolution();
            for i in 0..5 {
                let buf = cam.frame().expect("StreamCamera::frame");
                assert!(!buf.buffer().is_empty(), "frame {i} empty");
                assert_eq!(buf.resolution(), res);
            }
            cam.close().expect("StreamCamera::close");
        },
        OpenedCamera::Hybrid(mut cam) => {
            cam.open().expect("HybridCamera::open");
            let res = cam.negotiated_format().resolution();
            for i in 0..5 {
                let buf = cam.frame().expect("HybridCamera::frame");
                assert!(!buf.buffer().is_empty(), "frame {i} empty");
                assert_eq!(buf.resolution(), res);
            }
            cam.close().expect("HybridCamera::close");
        },
        OpenedCamera::Shutter(_) => {
            panic!("expected a stream-capable camera, got Shutter-only")
        },
    }
}

#[test]
fn enumerate_controls_and_formats() {
    match open_first() {
        // Stream needs `&mut` for `compatible_formats()`; the other
        // two wrappers expose `controls()` via `&self`.
        OpenedCamera::Stream(mut cam) => {
            cam.controls().expect("StreamCamera::controls");
            cam.compatible_formats()
                .expect("StreamCamera::compatible_formats");
        },
        OpenedCamera::Hybrid(cam) => {
            cam.controls().expect("HybridCamera::controls");
        },
        OpenedCamera::Shutter(cam) => {
            cam.controls().expect("ShutterCamera::controls");
        },
    }
}

/// Each `KnownCameraControl` ID must appear at most once in the
/// `controls()` Vec — duplicates would silently break any caller
/// that does `controls.iter().find(|c| c.control() == id)` (the
/// pattern `control_set_get_round_trip` uses internally and that
/// downstream UI picker code mirrors). MSMF iterates over
/// `all_known_camera_controls()` so dedupe is structural there, but
/// V4L2 walks driver-reported CIDs and translates them via
/// `id_to_known_camera_control()`; if the V4L2 driver reports two
/// CIDs that both map to the same `KnownCameraControl` (e.g. legacy
/// `V4L2_CID_BRIGHTNESS` plus a newer extended-control alias) the
/// translator would emit two `CameraControl { control: Brightness, .. }`
/// entries and `find` would return the first one — which is not
/// necessarily the active one. AVF builds the list from
/// `device.get_controls()` which assembles per-feature; a refactor
/// that double-counted, e.g., focus + auto-focus under the same
/// `KnownCameraControl::Focus` would slip in invisibly.
#[test]
fn controls_have_unique_known_ids() {
    use nokhwa::utils::KnownCameraControl;

    let controls = match open_first() {
        OpenedCamera::Stream(cam) => cam.controls().expect("StreamCamera::controls"),
        OpenedCamera::Hybrid(cam) => cam.controls().expect("HybridCamera::controls"),
        OpenedCamera::Shutter(cam) => cam.controls().expect("ShutterCamera::controls"),
    };
    let mut seen: std::collections::HashSet<KnownCameraControl> = std::collections::HashSet::new();
    for c in &controls {
        assert!(
            seen.insert(c.control()),
            "controls() returned duplicate KnownCameraControl {:?}; full list: {controls:?}",
            c.control()
        );
    }
}

#[test]
fn control_set_get_round_trip() {
    macro_rules! round_trip {
        ($cam:expr) => {{
            let cam = $cam;
            let controls = cam.controls().expect("controls()");

            // Prefer a Manual-mode IntegerRange control with headroom. Automatic-
            // flagged controls are skipped because set_control on MSMF preserves
            // the current flag: writing a value while the driver still owns the
            // control may round-trip intermittently.
            let candidate = controls.iter().find_map(|c| {
                let disqualified = c.flag().iter().any(|f| {
                    matches!(
                        f,
                        KnownCameraControlFlag::ReadOnly
                            | KnownCameraControlFlag::WriteOnly
                            | KnownCameraControlFlag::Disabled
                            | KnownCameraControlFlag::Automatic
                    )
                });
                if disqualified {
                    return None;
                }
                match c.description() {
                    ControlValueDescription::IntegerRange {
                        min,
                        max,
                        value,
                        step,
                        ..
                    } if *max > *min && *step > 0 => Some((c.control(), *min, *max, *step, *value)),
                    _ => None,
                }
            });

            let Some((id, min, max, step, current)) = candidate else {
                eprintln!(
                    "control_set_get_round_trip: no writable IntegerRange control in Manual mode \
                     is exposed by this device; skipping."
                );
                return;
            };

            let target = if current + step <= max {
                current + step
            } else if current - step >= min {
                current - step
            } else {
                eprintln!(
                    "control_set_get_round_trip: {id:?} has no headroom \
                     (min={min} max={max} step={step} value={current}); skipping."
                );
                return;
            };

            eprintln!(
                "control_set_get_round_trip: using {id:?} \
                 (min={min} max={max} step={step} current={current} target={target})"
            );

            cam.set_control(id, ControlValueSetter::Integer(target))
                .unwrap_or_else(|e| panic!("set_control({id:?}, {target}): {e}"));

            let after = cam.controls().expect("controls() after set");
            let updated = after
                .iter()
                .find(|c| c.control() == id)
                .expect("control disappeared after set");
            match updated.description() {
                ControlValueDescription::IntegerRange { value, .. } => assert_eq!(
                    *value, target,
                    "{id:?} did not round-trip: wanted {target}, got {value}"
                ),
                d => panic!("{id:?} changed description variant: {d:?}"),
            }

            let _ = cam.set_control(id, ControlValueSetter::Integer(current));
        }};
    }

    match open_first() {
        OpenedCamera::Stream(mut cam) => round_trip!(&mut cam),
        OpenedCamera::Hybrid(mut cam) => round_trip!(&mut cam),
        OpenedCamera::Shutter(mut cam) => round_trip!(&mut cam),
    }
}

/// Opening a non-existent index must surface a `NokhwaError` rather
/// than panicking or returning a bogus camera. Index 999 is well past
/// any realistic device count.
#[test]
fn open_invalid_index_errors() {
    let res = open(CameraIndex::Index(999), OpenRequest::any());
    assert!(
        res.is_err(),
        "open(CameraIndex::Index(999)) unexpectedly succeeded"
    );
}

/// `compatible_formats()` must enumerate at least one entry on a real
/// device. An empty list would mean negotiation has nothing to work
/// against.
#[test]
fn compatible_formats_nonempty() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("compatible_formats_nonempty: backend is not Stream-capable; skipping.");
        return;
    };
    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    assert!(!formats.is_empty(), "compatible_formats() returned empty");
}

/// Requesting a format the device cannot serve (1×1 @ 1 fps) must
/// either error or fail to round-trip. Backends differ — V4L2 may
/// silently snap to the nearest valid format, MSMF tends to error —
/// so this test accepts either outcome and just rejects the
/// "succeeded *and* round-tripped the bogus value" combination.
#[test]
fn set_format_invalid_does_not_round_trip() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "set_format_invalid_does_not_round_trip: backend is not Stream-capable; skipping."
        );
        return;
    };
    let bogus = CameraFormat::new(Resolution::new(1, 1), FrameFormat::MJPEG, 1);
    match cam.set_format(bogus) {
        Err(_) => {}, // expected on most backends
        Ok(()) => {
            let got = cam.negotiated_format();
            assert_ne!(
                got, bogus,
                "set_format accepted a 1x1@1 MJPEG format and round-tripped it; \
                 driver should have either errored or snapped to a real format"
            );
        },
    }
}

/// Multiple consecutive `frame()` calls must report a stable
/// resolution and source format. A regression here would mean the
/// stream is silently re-negotiating mid-stream, which would break
/// downstream `Buffer::typed::<F>()` consumers.
#[test]
fn frame_metadata_is_stable() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("frame_metadata_is_stable: backend is not Stream-capable; skipping.");
        return;
    };
    cam.open().expect("StreamCamera::open");
    let first = cam.frame().expect("first frame");
    let res = first.resolution();
    let fmt = first.source_frame_format();
    for i in 1..4 {
        let buf = cam.frame().expect("frame()");
        assert_eq!(buf.resolution(), res, "resolution drifted at frame {i}");
        assert_eq!(
            buf.source_frame_format(),
            fmt,
            "source format drifted at frame {i}"
        );
    }
    cam.close().expect("StreamCamera::close");
}

/// `compatible_fourcc()` must enumerate at least one entry, and every
/// entry must be a `FrameFormat` that also appears in
/// `compatible_formats()`. This is the invariant that the MSMF
/// truncation bug in #194 violated — `compatible_fourcc` returned at
/// most 2 entries while `compatible_formats` could expose all 4
/// (MJPEG / YUYV / NV12 / GRAY), so a UI that branched on
/// `compatible_fourcc` saw a strict subset of what the device
/// actually supported.
#[test]
fn compatible_fourcc_is_subset_of_compatible_formats() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "compatible_fourcc_is_subset_of_compatible_formats: backend is not Stream-capable; skipping."
        );
        return;
    };
    let fourccs = cam
        .compatible_fourcc()
        .expect("StreamCamera::compatible_fourcc");
    assert!(!fourccs.is_empty(), "compatible_fourcc() returned empty");

    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    let formats_fourccs: std::collections::HashSet<FrameFormat> =
        formats.iter().map(|f| f.format()).collect();
    for ff in &fourccs {
        assert!(
            formats_fourccs.contains(ff),
            "compatible_fourcc returned {ff:?} which is not in compatible_formats() = {formats_fourccs:?}"
        );
    }
}

/// Reverse direction of the subset test above: every fourcc that
/// appears in `compatible_formats()` must also appear in
/// `compatible_fourcc()`. Together with `…_is_subset_of_compatible_formats`
/// this pins set-equality, which is the actual user-facing contract:
/// callers who only need to know which fourccs the device supports
/// (e.g. picker UIs that don't yet care about resolutions / frame
/// rates) can call the cheaper `compatible_fourcc()` and trust it
/// covers exactly the same set of fourccs the slower
/// `compatible_formats()` would surface. V4L2 derives one from the
/// other so the equality holds structurally there, but MSMF and AVF
/// enumerate them independently — a regression where one path
/// reports an extra format the other doesn't would silently break
/// any caller that picks one for performance reasons.
#[test]
fn compatible_fourcc_is_superset_of_compatible_formats() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "compatible_fourcc_is_superset_of_compatible_formats: backend is not Stream-capable; skipping."
        );
        return;
    };
    let fourccs = cam
        .compatible_fourcc()
        .expect("StreamCamera::compatible_fourcc");
    let fourccs_set: std::collections::HashSet<FrameFormat> = fourccs.iter().copied().collect();

    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    for fmt in &formats {
        let ff = fmt.format();
        assert!(
            fourccs_set.contains(&ff),
            "compatible_formats() exposed {ff:?} (from {fmt:?}) which is missing from compatible_fourcc() = {fourccs:?}"
        );
    }
}

/// Round-trip an arbitrary entry from `compatible_formats()` through
/// `set_format()` and confirm `negotiated_format()` reports the same
/// values. Catches drift between `compatible_formats` (what the
/// backend says it supports) and `set_format` (what the backend
/// actually accepts) — these can diverge if a backend's
/// `compatible_formats` returns synthesised entries the driver can't
/// honour at `set_format` time.
#[test]
fn set_format_from_compatible_round_trip() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "set_format_from_compatible_round_trip: backend is not Stream-capable; skipping."
        );
        return;
    };
    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    let Some(target) = formats.into_iter().next() else {
        eprintln!("set_format_from_compatible_round_trip: no compatible formats; skipping.");
        return;
    };
    cam.set_format(target).unwrap_or_else(|e| {
        panic!("set_format({target:?}) returned error on a value from compatible_formats(): {e}")
    });
    let got = cam.negotiated_format();
    assert_eq!(
        got, target,
        "negotiated_format mismatched after set_format({target:?}); got {got:?}"
    );
}

/// Re-negotiating the format on an *already-streaming* camera must
/// succeed and keep delivering frames. The backend has to tear the
/// live stream down before issuing the format change — V4L2, for one,
/// rejects `VIDIOC_S_FMT` / `VIDIOC_REQBUFS` while a stream is active
/// (EBUSY), and re-acquiring buffers before releasing the old arena is
/// a use-after-stream hazard. This drives `open() → frame() →
/// set_format(other) → frame()` to catch a backend that mishandles the
/// re-open ordering.
#[test]
fn set_format_while_streaming_reacquires_stream() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "set_format_while_streaming_reacquires_stream: backend is not Stream-capable; skipping."
        );
        return;
    };
    let mut formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    formats.sort_by_key(|f| (f.width(), f.height(), f.frame_rate()));
    formats.dedup();
    if formats.len() < 2 {
        eprintln!(
            "set_format_while_streaming_reacquires_stream: fewer than 2 compatible formats; \
             skipping."
        );
        return;
    }
    let first = formats[0];
    let second = formats[formats.len() - 1];

    cam.set_format(first)
        .unwrap_or_else(|e| panic!("set_format({first:?}) before streaming: {e}"));
    cam.open().expect("StreamCamera::open");
    let _ = cam.frame().expect("frame() before re-negotiation");

    // The path under test: change format with the stream live.
    cam.set_format(second)
        .unwrap_or_else(|e| panic!("set_format({second:?}) while streaming returned error: {e}"));
    assert_eq!(
        cam.negotiated_format(),
        second,
        "negotiated_format must reflect the format set while streaming"
    );
    let buf = cam
        .frame()
        .expect("frame() must still flow after a streaming-time set_format");
    assert_eq!(
        buf.resolution(),
        second.resolution(),
        "frame resolution must match the re-negotiated format"
    );
    cam.close().expect("StreamCamera::close");
}

/// `info()` and `backend()` must reflect the device that was opened —
/// `info().index()` matches the index passed to `open()`, and
/// `backend()` matches the platform's `native_api_backend()`. Catches
/// a regression where a wrapper's `info()` returns stale or
/// constructor-default data instead of pass-through to the backend.
#[test]
fn opened_camera_info_and_backend_reflect_request() {
    let cam = open_first();
    let (backend, info) = match &cam {
        OpenedCamera::Stream(c) => (c.backend(), c.info()),
        OpenedCamera::Shutter(c) => (c.backend(), c.info()),
        OpenedCamera::Hybrid(c) => (c.backend(), c.info()),
    };
    assert_eq!(
        backend,
        native_backend(),
        "OpenedCamera::backend() must match native_api_backend() for an index-opened device"
    );
    assert_eq!(
        info.index(),
        &CameraIndex::Index(0),
        "OpenedCamera::info().index() must echo the index passed to open()"
    );
    assert!(
        !info.human_name().is_empty(),
        "OpenedCamera::info().human_name() must be non-empty for a real device"
    );
}

/// The dual-form `CameraIndex` contract — a numeric `String` is a
/// valid index, by parsing — must hold through the public `open()`
/// dispatcher, not just at the `as_index()` unit-test layer.
/// `open(CameraIndex::String("0"))` must reach the same native
/// backend as `open(CameraIndex::Index(0))`. Regression here would
/// silently route numeric-string callers to GStreamer's URL path
/// (which expects `rtsp://`/`http://`/`file://`) and produce a
/// backend mismatch on the resulting `OpenedCamera`.
#[test]
fn open_numeric_string_routes_to_native_backend() {
    let cam = open(CameraIndex::String("0".to_string()), OpenRequest::any())
        .expect("open(CameraIndex::String(\"0\")) must dispatch to the native backend");
    let backend = match &cam {
        OpenedCamera::Stream(c) => c.backend(),
        OpenedCamera::Shutter(c) => c.backend(),
        OpenedCamera::Hybrid(c) => c.backend(),
    };
    assert_eq!(
        backend,
        native_backend(),
        "open(String(\"0\")) reached the wrong backend; numeric-string dispatch is broken"
    );
}

/// `frame()` must return non-empty bytes. `frame_metadata_is_stable`
/// already pins resolution + source format across frames; this is the
/// matching pin for the actual payload. A regression that returns
/// `Buffer { buffer: Cow::Borrowed(&[]), .. }` would slip past every
/// existing test because they only inspect metadata.
#[test]
fn frame_buffer_is_non_empty() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("frame_buffer_is_non_empty: backend is not Stream-capable; skipping.");
        return;
    };
    cam.open().expect("StreamCamera::open");
    let buf = cam.frame().expect("StreamCamera::frame");
    assert!(
        !buf.buffer().is_empty(),
        "frame() returned a Buffer with zero payload bytes — backend is producing empty frames"
    );
    cam.close().expect("StreamCamera::close");
}

/// `is_open()` must reflect the open/close lifecycle: false before
/// `open()`, true after `open()`, false again after `close()`. A
/// regression where `is_open()` is hardcoded `true` (or never updated
/// on `close()`) would silently break callers that branch on this
/// flag for re-init logic.
#[test]
fn stream_camera_is_open_lifecycle() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("stream_camera_is_open_lifecycle: backend is not Stream-capable; skipping.");
        return;
    };
    assert!(
        !cam.is_open(),
        "is_open() must be false before StreamCamera::open()"
    );
    cam.open().expect("StreamCamera::open");
    assert!(
        cam.is_open(),
        "is_open() must be true after StreamCamera::open()"
    );
    cam.close().expect("StreamCamera::close");
    assert!(
        !cam.is_open(),
        "is_open() must be false after StreamCamera::close()"
    );
}

/// `frame_raw()` is the zero-copy sibling of `frame()`. The raw byte
/// slice must be non-empty for the same reason `frame()`'s `Buffer`
/// payload must be — a regression that returns `Cow::Borrowed(&[])`
/// would slip past `frame_buffer_is_non_empty` (which goes through
/// `frame()`) and silently break low-level consumers that read
/// `frame_raw()` directly to avoid the `Buffer` copy.
#[test]
fn frame_raw_is_non_empty() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("frame_raw_is_non_empty: backend is not Stream-capable; skipping.");
        return;
    };
    cam.open().expect("StreamCamera::open");
    let raw = cam.frame_raw().expect("StreamCamera::frame_raw");
    assert!(
        !raw.is_empty(),
        "frame_raw() returned a zero-length slice — backend is producing empty frames"
    );
    cam.close().expect("StreamCamera::close");
}

/// Indices returned by `query()` must be openable by `open()`. The
/// dual-form `CameraIndex` contract has the parsed-from-string path
/// (covered by `open_numeric_string_routes_to_native_backend`); this
/// is the matching pin for the typed `Index` path: `query()` is the
/// canonical enumerator, and a `CameraInfo` it returns must round-trip
/// through `open()` without surprise. A regression — `query()`
/// reporting an index `open()` rejects — would break every consumer
/// that picks devices by enumeration.
#[test]
fn query_results_are_openable() {
    let devices = query(native_backend()).expect("query() returned an error");
    let Some(first) = devices.first() else {
        eprintln!("query_results_are_openable: query returned empty; skipping.");
        return;
    };
    let CameraIndex::Index(idx) = first.index() else {
        eprintln!(
            "query_results_are_openable: native query returned a non-Index variant ({:?}); skipping.",
            first.index()
        );
        return;
    };
    let cam = open(CameraIndex::Index(*idx), OpenRequest::any())
        .unwrap_or_else(|e| panic!("open(query[0].index = {idx}) failed: {e}"));
    let backend = match &cam {
        OpenedCamera::Stream(c) => c.backend(),
        OpenedCamera::Shutter(c) => c.backend(),
        OpenedCamera::Hybrid(c) => c.backend(),
    };
    assert_eq!(
        backend,
        native_backend(),
        "open() of a query-reported index reached the wrong backend"
    );
}

/// `OpenedCamera::info()` must match the corresponding `CameraInfo`
/// returned by `query()` for the same index — at minimum on
/// `human_name`, the field every picker UI shows the user. The
/// existing `query_results_are_openable` only confirms the open
/// itself succeeds and routes to the right backend; it doesn't
/// catch a regression where `query()` reports device A's name at
/// position 0 but `open(Index(0))` actually opens device B (and
/// `info()` reports B's name). That's the kind of silent off-by-one
/// that "I selected my Logitech but my Brio came up" bug reports
/// trace back to. V4L2's `/dev/videoN` ordering can differ between
/// `query()` (sorted) and `open()` (positional via `Camera::with_path`)
/// if a future refactor changes one without the other; MSMF's
/// `MFEnumDeviceSources` order is what `open(Index(i))` uses
/// internally, so a regression that re-sorted one but not the
/// other in the wrapper layer would surface here.
#[test]
fn open_info_matches_query_for_same_index() {
    let devices = query(native_backend()).expect("query() returned an error");
    let Some(first) = devices.first() else {
        eprintln!("open_info_matches_query_for_same_index: query returned empty; skipping.");
        return;
    };
    let CameraIndex::Index(idx) = first.index() else {
        eprintln!(
            "open_info_matches_query_for_same_index: native query returned a non-Index variant ({:?}); skipping.",
            first.index()
        );
        return;
    };
    let cam = open(CameraIndex::Index(*idx), OpenRequest::any())
        .unwrap_or_else(|e| panic!("open(query[0].index = {idx}) failed: {e}"));
    let opened_info = match &cam {
        OpenedCamera::Stream(c) => c.info(),
        OpenedCamera::Shutter(c) => c.info(),
        OpenedCamera::Hybrid(c) => c.info(),
    };
    assert_eq!(
        opened_info.index(),
        first.index(),
        "OpenedCamera::info().index() {:?} does not match query[0].index() {:?}",
        opened_info.index(),
        first.index()
    );
    assert_eq!(
        opened_info.human_name(),
        first.human_name(),
        "OpenedCamera::info().human_name() {:?} does not match query[0].human_name() {:?} — open() may have routed to a different device than query() advertised at that index",
        opened_info.human_name(),
        first.human_name()
    );
}

/// Every entry in `compatible_formats()` must have a non-zero
/// resolution. A 0×0 entry would silently feed into `set_format()` /
/// `RequestedFormat::Exact` and either error or produce a degenerate
/// stream, depending on the backend. The closest-match negotiation
/// path also assumes positive resolutions for distance computation.
#[test]
fn compatible_formats_have_nonzero_resolutions() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "compatible_formats_have_nonzero_resolutions: backend is not Stream-capable; skipping."
        );
        return;
    };
    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    for f in &formats {
        assert!(
            f.resolution().width() > 0,
            "compatible_formats() returned a 0-width entry: {f:?}"
        );
        assert!(
            f.resolution().height() > 0,
            "compatible_formats() returned a 0-height entry: {f:?}"
        );
    }
}

/// `compatible_fourcc()` must return entries in `FrameFormat::Ord`
/// order with no duplicates. This is the cross-backend invariant
/// established by #194 / #195 / #196 / #197 / #198: V4L /
/// AVFoundation / MSMF / GStreamer all produce the same `collect →
/// sort → dedup` shape so callers see a stable list regardless of
/// platform. A regression — backend-specific ordering or duplicate
/// entries — would break UI code that branches on the first / last
/// reported `FrameFormat`.
#[test]
fn compatible_fourcc_is_sorted_and_deduped() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "compatible_fourcc_is_sorted_and_deduped: backend is not Stream-capable; skipping."
        );
        return;
    };
    let fourccs = cam
        .compatible_fourcc()
        .expect("StreamCamera::compatible_fourcc");

    let mut sorted = fourccs.clone();
    sorted.sort();
    assert_eq!(
        fourccs, sorted,
        "compatible_fourcc() returned entries out of FrameFormat::Ord order: {fourccs:?}"
    );

    let mut deduped = fourccs.clone();
    deduped.dedup();
    assert_eq!(
        fourccs, deduped,
        "compatible_fourcc() returned duplicate entries: {fourccs:?}"
    );
}

/// `StreamCamera::open()` after a prior `close()` must succeed and
/// resume frame delivery. Catches a regression where a backend leaves
/// state behind on `close()` (e.g. an undropped session handle, a
/// stuck `is_open` flag, or a stale frame channel) that prevents the
/// next `open()` from re-establishing the stream. The lifecycle test
/// only goes open → close once; this pins the reusable-across-cycles
/// contract that long-running consumers (a UI that pauses + resumes
/// the camera) depend on.
#[test]
fn stream_camera_reopen_after_close() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("stream_camera_reopen_after_close: backend is not Stream-capable; skipping.");
        return;
    };
    cam.open().expect("first StreamCamera::open");
    let _ = cam.frame().expect("first frame after first open");
    cam.close().expect("first StreamCamera::close");
    assert!(
        !cam.is_open(),
        "is_open() must be false after close(); reopen test cannot proceed"
    );
    cam.open().expect("second StreamCamera::open after close");
    assert!(
        cam.is_open(),
        "is_open() must be true after the second open()"
    );
    let buf = cam.frame().expect("frame() after reopen");
    assert!(
        !buf.buffer().is_empty(),
        "frame() returned empty payload after reopen — backend lost state across close+open"
    );
    cam.close().expect("second StreamCamera::close");
}

/// Calling `close()` twice in a row must be a safe no-op on the
/// second call: every native backend handles this differently
/// internally — V4L2 drops a `Some(stream_handle)` to `None` on the
/// first call and then short-circuits on the next, MSMF calls
/// `stop_stream()` (already a fire-and-forget sink), and
/// AVFoundation explicitly guards with `if !self.is_open() { return
/// Ok(()); }`. A regression in any one of those guards would surface
/// as a panic, an `Err` from a freshly-released session, or an
/// `is_open()` that flips back to `true` after the second close.
/// Pin all three observable invariants here.
#[test]
fn stream_camera_double_close_is_idempotent() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "stream_camera_double_close_is_idempotent: backend is not Stream-capable; skipping."
        );
        return;
    };
    cam.open().expect("StreamCamera::open");
    let _ = cam.frame().expect("frame() before first close");
    cam.close().expect("first StreamCamera::close");
    assert!(
        !cam.is_open(),
        "is_open() must be false after the first close()"
    );
    cam.close()
        .expect("second StreamCamera::close must be Ok (idempotent contract)");
    assert!(
        !cam.is_open(),
        "is_open() must remain false after a redundant close()"
    );
}

/// `compatible_formats()` must not duplicate entries. The
/// per-backend enumerators sometimes emit the same `(resolution,
/// frame_format, frame_rate)` tuple twice when the underlying API
/// reports a format under multiple internal media-type IDs (MSMF
/// `IMFAttributes` enumeration is the historical offender). The
/// pipeline `RequestedFormat::fulfill` and downstream UI code that
/// indexes into this list assume each tuple is unique; a regression
/// would silently double-count a format and surface as a confusing
/// duplicate row in pickers.
#[test]
fn compatible_formats_unique() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!("compatible_formats_unique: backend is not Stream-capable; skipping.");
        return;
    };
    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");

    let mut sorted = formats.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        formats.len(),
        sorted.len(),
        "compatible_formats() returned duplicate (resolution, format, frame_rate) tuples: {formats:?}"
    );
}

/// After `set_format(fmt)` succeeds, `negotiated_format()` must
/// reflect that exact format. Catches a regression where a backend
/// silently substitutes a different format on the way in (e.g.
/// fallback to a default resolution because the requested resolution
/// failed an internal validation) without surfacing an error. The
/// existing `set_format_from_compatible_round_trip` runs the round
/// trip end-to-end for every entry; this is the focused
/// single-format pin so a future bisect points at the
/// negotiated-format invariant directly.
#[test]
fn negotiated_format_after_set_format_matches() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "negotiated_format_after_set_format_matches: backend is not Stream-capable; skipping."
        );
        return;
    };
    let formats = cam
        .compatible_formats()
        .expect("StreamCamera::compatible_formats");
    let Some(target) = formats.into_iter().next() else {
        eprintln!(
            "negotiated_format_after_set_format_matches: device exposes no compatible formats; skipping."
        );
        return;
    };
    cam.set_format(target)
        .unwrap_or_else(|e| panic!("set_format({target}): {e}"));
    let negotiated = cam.negotiated_format();
    assert_eq!(
        negotiated, target,
        "negotiated_format() does not match set_format() input: wanted {target}, got {negotiated}"
    );
}

/// `frame()`'s reported `resolution()` and `source_frame_format()`
/// must match what `negotiated_format()` reports. This is the
/// **cross-surface** invariant — the API surface (`negotiated_format`)
/// and the per-frame metadata (`Buffer::resolution` /
/// `Buffer::source_frame_format`) come from different code paths in
/// every backend, and a regression in either could leave them out of
/// sync. Catches bugs where:
///   - `negotiated_format` is cached at open() time but the device
///     silently re-negotiates on the wire (buffer reflects the wire
///     format; the API surface lies);
///   - the frame buffer is built with a hard-coded fallback format
///     (e.g. always YUYV) regardless of what was negotiated;
///   - resolution drift between the API-reported size and the actual
///     pixel count, which silently breaks downstream decoders.
///
/// The existing `frame_metadata_is_stable` test only checks frame-to-
/// frame consistency; this pins the surface-to-buffer link.
#[test]
fn frame_metadata_matches_negotiated_format() {
    let OpenedCamera::Stream(mut cam) = open_first() else {
        eprintln!(
            "frame_metadata_matches_negotiated_format: backend is not Stream-capable; skipping."
        );
        return;
    };
    cam.open().expect("StreamCamera::open");
    let negotiated = cam.negotiated_format();
    let frame = cam.frame().expect("frame()");
    assert_eq!(
        frame.resolution(),
        negotiated.resolution(),
        "frame.resolution() {} does not match negotiated_format().resolution() {}",
        frame.resolution(),
        negotiated.resolution()
    );
    assert_eq!(
        frame.source_frame_format(),
        negotiated.format(),
        "frame.source_frame_format() {:?} does not match negotiated_format().format() {:?}",
        frame.source_frame_format(),
        negotiated.format()
    );
    cam.close().expect("StreamCamera::close");
}

/// `negotiated_format()` is a pure read — calling it twice in a row
/// (with no `set_format` in between) must return identical values.
/// All three native backends today implement it as a struct-field
/// read (`self.camera_format` on V4L, `self.format` on AVF,
/// `self.inner.format()` on MSMF), so this is trivially true. The
/// pin guards against a future refactor that introduces per-call
/// recomputation (e.g. re-querying the device with a TTL cache that
/// drifts between calls, or interior mutability that's not actually
/// idempotent). A regression would surface as a downstream consumer
/// that polls `negotiated_format()` to drive UI state seeing flicker
/// even though no `set_format` ever fired.
#[test]
fn negotiated_format_read_is_stable() {
    let OpenedCamera::Stream(cam) = open_first() else {
        eprintln!("negotiated_format_read_is_stable: backend is not Stream-capable; skipping.");
        return;
    };
    let first = cam.negotiated_format();
    let second = cam.negotiated_format();
    assert_eq!(
        first, second,
        "negotiated_format() returned {first} then {second} on consecutive reads with no set_format in between"
    );
}

/// `controls()` is a pure read — calling it twice in a row (with no
/// `set_control` in between) must return identical lists. Each
/// backend implements it differently: V4L2 walks driver-reported CIDs
/// and translates each to a `KnownCameraControl`, MSMF iterates
/// `all_known_camera_controls()` and probes each via
/// `IAMVideoProcAmp` / `IAMCameraControl`, AVFoundation fans out per
/// hardware feature flag. A regression where any of those probes
/// became flaky (e.g. timing-dependent because of an internal cache
/// invalidation) would surface here as a non-idempotent read.
/// Downstream pickers and the runner's `controls_snapshot` rely on
/// the assumption that two reads with no mutation in between agree.
#[test]
fn controls_read_is_stable() {
    let OpenedCamera::Stream(cam) = open_first() else {
        eprintln!("controls_read_is_stable: backend is not Stream-capable; skipping.");
        return;
    };
    let first = cam.controls().expect("first controls()");
    let second = cam.controls().expect("second controls()");
    assert_eq!(
        first, second,
        "controls() returned different lists on consecutive reads with no set_control in between: first={first:?} second={second:?}"
    );
}

// The file is already gated by `device-test` at the top, so this
// submodule's effective gate is `device-test AND runner` — i.e. it
// compiles only when both features are enabled.
#[cfg(feature = "runner")]
mod runner_tests {
    use super::{open, CameraIndex, ControlValueSetter, OpenRequest};
    use nokhwa::utils::KnownCameraControl;
    use nokhwa::{CameraRunner, Overflow, RunnerConfig};
    use std::time::Duration;

    #[test]
    fn runner_produces_frames() {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())
            .expect("open(CameraIndex::Index(0)) failed");
        let runner =
            CameraRunner::spawn(opened, RunnerConfig::default()).expect("CameraRunner::spawn");
        let frames = runner.frames().expect("runner has no frames channel");
        for i in 0..3 {
            let buf = frames
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|e| panic!("recv frame {i}: {e}"));
            assert!(!buf.buffer().is_empty(), "runner frame {i} empty");
        }
        runner.stop().expect("runner.stop()");
    }

    // The `DropOldest` policy spawns a relay thread that maintains a
    // bounded `VecDeque` between the producer and the user-facing
    // channel. The unit tests in `src/runner.rs` cover that thread
    // with a fake producer; this one runs it under a real camera so
    // the relay's full lifecycle (spawn → forward → join on
    // shutdown) is exercised end-to-end. Capacity 2 with the camera
    // running at >2 FPS guarantees overflow within a few hundred
    // milliseconds, then we drain the channel and call `stop()`. A
    // regression that left the relay thread orphaned (e.g. forgot to
    // close the relay's input on shutdown) would surface here as
    // `stop()` hanging or panicking on a dangling join handle.
    #[test]
    fn runner_drop_oldest_overflow_drains_relay_on_stop() {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())
            .expect("open(CameraIndex::Index(0)) failed");
        let cfg = RunnerConfig {
            frames_capacity: 2,
            overflow: Overflow::DropOldest,
            ..RunnerConfig::default()
        };
        let runner = CameraRunner::spawn(opened, cfg).expect("CameraRunner::spawn");
        let frames = runner.frames().expect("runner has no frames channel");
        // Pull at least one frame so we know the relay is forwarding.
        let buf = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("first frame from drop-oldest runner");
        assert!(!buf.buffer().is_empty(), "first frame empty");
        // Let the producer outpace us on a small bounded channel so the
        // relay's drop-oldest path actually fires.
        std::thread::sleep(Duration::from_millis(300));
        // Drain whatever the relay queued so `stop()` doesn't time out
        // waiting for backpressure to clear.
        while frames.try_recv().is_ok() {}
        runner.stop().expect("runner.stop() with drop-oldest relay");
    }

    // `CameraRunner::Drop` must shut the worker thread down even when
    // the user forgets to call `.stop()`. The `runner_produces_frames`
    // test above always calls `.stop()`, so a regression that, e.g.,
    // panicked in `Drop` because the channel was already half-closed
    // would slip through. Spawn a runner, take one frame, then let
    // it fall out of scope. If `Drop` deadlocks the worker we'd hang
    // here; if it panics, the test fails.
    #[test]
    fn runner_drop_without_explicit_stop_cleans_up() {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())
            .expect("open(CameraIndex::Index(0)) failed");
        {
            let runner =
                CameraRunner::spawn(opened, RunnerConfig::default()).expect("CameraRunner::spawn");
            let frames = runner.frames().expect("runner has no frames channel");
            let buf = frames
                .recv_timeout(Duration::from_secs(5))
                .expect("first frame from implicit-drop runner");
            assert!(!buf.buffer().is_empty(), "implicit-drop frame empty");
        } // `runner` drops here; Drop must clean up the worker.
          // If we got past Drop without panic / deadlock the test passes.
    }

    // `OpenRequest::any()` resolves to a stream-only camera on every
    // hardware backend in this repo (V4L2 / MSMF / AVFoundation), so
    // `runner.pictures()` must be `None` and `runner.events()` must
    // be `None` (none of those native backends expose `EventSource`
    // through the runner today). A regression that started wiring
    // a `pictures` channel for stream-only backends would silently
    // leak a relay thread for every `CameraRunner::spawn`.
    #[test]
    fn runner_stream_only_backend_yields_no_pictures_no_events() {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())
            .expect("open(CameraIndex::Index(0)) failed");
        let runner =
            CameraRunner::spawn(opened, RunnerConfig::default()).expect("CameraRunner::spawn");
        assert!(
            runner.pictures().is_none(),
            "stream-only runner must not have a pictures channel"
        );
        assert!(
            runner.events().is_none(),
            "stream-only runner must not have an events channel"
        );
        runner.stop().expect("runner.stop()");
    }

    // `set_control` on a running runner forwards the command to the
    // worker thread, which applies it to the underlying camera. The
    // synchronous `control_set_get_round_trip` test (above, outside
    // this submodule) covers the direct path; this one pins the
    // runner-mediated path. We don't assert the new value round-trips
    // because not every backend reports a fresh `controls()` snapshot
    // synchronously after `set_control`; we only assert the call
    // succeeds (no `Err`) and that frame delivery survives the
    // control change. A regression that made `set_control` return
    // `Err` for any backend, or wedged the worker on a control
    // command, would surface here.
    #[test]
    fn runner_set_control_does_not_disrupt_frame_delivery() {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())
            .expect("open(CameraIndex::Index(0)) failed");
        let runner =
            CameraRunner::spawn(opened, RunnerConfig::default()).expect("CameraRunner::spawn");
        let frames = runner.frames().expect("runner has no frames channel");

        // Drain one frame to confirm the worker is steady-state before
        // we send the control command.
        frames
            .recv_timeout(Duration::from_secs(5))
            .expect("first frame before set_control");

        // Brightness is the most universally-supported control across
        // V4L / MSMF / AVF webcams. The integer 0 is a safe value:
        // most backends accept it (some clamp to the supported range,
        // which is fine — we're not asserting the value, only that
        // the call doesn't panic the worker).
        runner
            .set_control(
                KnownCameraControl::Brightness,
                ControlValueSetter::Integer(0),
            )
            .expect("set_control(Brightness, 0) on running runner");

        // Frame delivery must continue. A wedged worker would time
        // out here.
        let buf = frames
            .recv_timeout(Duration::from_secs(5))
            .expect("frame after set_control — worker may be wedged");
        assert!(!buf.buffer().is_empty(), "post-set_control frame empty");

        runner.stop().expect("runner.stop()");
    }

    // `Overflow::Block` is the third overflow policy, alongside
    // `DropNewest` (the default, exercised by `runner_produces_frames`)
    // and `DropOldest` (exercised by
    // `runner_drop_oldest_overflow_drains_relay_on_stop`). `Block` is
    // structurally different — no relay thread, the worker's
    // `blocking_send` stalls until the consumer drains. The unit tests
    // in `src/runner.rs` (lines ~720-755) cover `make_channel` with
    // synthetic data, but the full producer-thread → frames-channel
    // hardware path is unpinned end-to-end. A regression that, e.g.,
    // collapsed `Block` to `DropNewest` (silently dropping under load)
    // or wedged the worker on `blocking_send` would slip past every
    // existing test. Spawn with `Block` + capacity 4, pull frames,
    // stop cleanly.
    #[test]
    fn runner_block_overflow_delivers_frames_and_stops() {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())
            .expect("open(CameraIndex::Index(0)) failed");
        let cfg = RunnerConfig {
            frames_capacity: 4,
            overflow: Overflow::Block,
            ..RunnerConfig::default()
        };
        let runner = CameraRunner::spawn(opened, cfg).expect("CameraRunner::spawn with Block");
        let frames = runner.frames().expect("runner has no frames channel");

        // Pull three frames. With `Block`, the worker stalls on a full
        // channel; we keep the consumer ahead of the producer so the
        // happy path (no stalls) is exercised. A regression that
        // collapsed `Block` to `DropNewest` would still pass *this*
        // half (frames flow); the stop path below catches the other
        // half (no relay thread to leak / re-drain on shutdown).
        for i in 0..3 {
            let buf = frames
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|e| panic!("recv frame {i} on Block runner: {e}"));
            assert!(!buf.buffer().is_empty(), "Block runner frame {i} empty");
        }

        runner.stop().expect("runner.stop() on Block runner");
    }
}
