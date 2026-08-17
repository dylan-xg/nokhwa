use super::*;

#[test]
fn resolution_new() {
    let res = Resolution::new(1920, 1080);
    assert_eq!(res.width(), 1920);
    assert_eq!(res.height(), 1080);
    assert_eq!(res.x(), 1920);
    assert_eq!(res.y(), 1080);
    assert_eq!(res.width_x, 1920);
    assert_eq!(res.height_y, 1080);
}

// `Resolution` derives `Default`, which initialises both fields to
// `0` (`u32::default()`). No test currently exercises this — a
// hand-written `impl Default for Resolution` introduced later with
// a tempting "useful default" like `640x480` would silently change
// the value seen by any consumer pre-allocating metadata before the
// first frame arrives (and by `CameraFormat::default()`, which
// composes `Resolution::default()`). Pin the contract.
#[test]
fn resolution_default_is_zero_zero() {
    let r = Resolution::default();
    assert_eq!(r.width(), 0);
    assert_eq!(r.height(), 0);
}

// `Resolution::Display` emits `"{w}x{h}"` (e.g. `"640x480"`).
// Previously this test used `contains("640")` + `contains("480")`,
// which would silently pass after a refactor that flipped the
// separator to `" x "` / `"×"`, swapped order to `HxW`, or
// re-arranged the form. `CameraFormat::Display` *delegates* to
// this impl (see `camera_format_display_renders_resolution_at_fps_then_format`),
// so any downstream log scraper that splits on `'x'` to recover
// the dimensions of a `CameraFormat` string depends on this exact
// shape. Pin it verbatim. Guards `nokhwa-core/src/types.rs:571-575`.
#[test]
fn resolution_display_exact_format() {
    let res = Resolution::new(640, 480);
    assert_eq!(format!("{res}"), "640x480");
}

#[test]
fn resolution_ordering() {
    let low = Resolution::new(640, 480);
    let mid = Resolution::new(1280, 720);
    let high = Resolution::new(1920, 1080);
    assert!(low < mid);
    assert!(mid < high);
    assert!(low < high);
}

#[test]
fn resolution_ordering_equal_width_falls_through_to_height() {
    let portrait_short = Resolution::new(1920, 1080);
    let portrait_tall = Resolution::new(1920, 2160);
    assert!(portrait_short < portrait_tall);
    assert!(portrait_tall > portrait_short);
    let dup = Resolution::new(1920, 1080);
    assert_eq!(portrait_short.cmp(&dup), std::cmp::Ordering::Equal);
}

#[test]
fn resolution_ordering_is_lexicographic_not_area() {
    // Width is the primary key, area is irrelevant — a 1×0
    // resolution must be greater than a 0×huge resolution. This
    // pins the lex-ascending contract that the upstream docstring
    // used to mis-describe ("flipped from highest to lowest" — the
    // code never matched) and that `RequestedFormat::fulfill`
    // relies on via `Iterator::max`. A regression that switched to
    // area-based ordering would silently change which resolution
    // `max()` picks for `AbsoluteHighestResolution` requests.
    let huge_height_zero_width = Resolution::new(0, u32::MAX);
    let one_pixel_wide = Resolution::new(1, 0);
    assert!(huge_height_zero_width < one_pixel_wide);
}

#[test]
fn resolution_iter_max_picks_largest_width_then_height() {
    // End-to-end pinning of the `Vec::iter().max()` path used by
    // `RequestedFormat::fulfill` for `AbsoluteHighestResolution`.
    // A comparator inversion (someone "fixing" the misleading
    // upstream "flipped" docstring by reversing `cmp`) would
    // silently make `fulfill` return the *lowest* resolution.
    let candidates = [
        Resolution::new(640, 480),
        Resolution::new(1920, 1080),
        Resolution::new(1280, 720),
        Resolution::new(1920, 2160),
        Resolution::new(1024, 768),
    ];
    let winner = candidates.iter().max().copied();
    assert_eq!(winner, Some(Resolution::new(1920, 2160)));
}

#[test]
fn resolution_equality() {
    let a = Resolution::new(640, 480);
    let b = Resolution::new(640, 480);
    let c = Resolution::new(1280, 720);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn frame_format_display_roundtrip() {
    for fmt in frame_formats() {
        let s = format!("{fmt}");
        let parsed: FrameFormat = s.parse().expect("should parse back");
        assert_eq!(*fmt, parsed);
    }
}

// `frame_format_display_roundtrip` is *symmetry-only*: it parses the
// Display output back via `FromStr` and checks round-trip equality, so
// it would pass even if every variant accidentally emitted the same
// string (as long as `FromStr` matched the same mapping) or two
// variants swapped their Display strings (as long as `FromStr`
// swapped to match). Only `RAWBGR` has a standalone exact pin
// (`frame_format_rawbgr_display_parse`); `CameraFormat::Display`
// indirectly pins `MJPEG` + `YUYV` via the embedded `"{format} Format"`
// rendering — but `GRAY`, `RAWRGB`, and `NV12` are not exact-pinned
// anywhere. The Display strings appear in `NokhwaError::ProcessFrameError`
// (`"... process frame {src} to {destination} ..."`), `ReadFrameError`
// (`"... (format {f}) ..."`), and `CameraFormat::Display`, all of which
// are user-visible. A refactor that renamed `RAWRGB` to `"RGB"` or
// `"RAW_RGB"` would survive the roundtrip but break the embedded
// renderings. Pin every variant verbatim. Guards
// `nokhwa-core/src/types.rs:394-416`.
#[test]
fn frame_format_display_exact_strings() {
    assert_eq!(FrameFormat::MJPEG.to_string(), "MJPEG");
    assert_eq!(FrameFormat::YUYV.to_string(), "YUYV");
    assert_eq!(FrameFormat::GRAY.to_string(), "GRAY");
    assert_eq!(FrameFormat::RAWRGB.to_string(), "RAWRGB");
    assert_eq!(FrameFormat::RAWBGR.to_string(), "RAWBGR");
    assert_eq!(FrameFormat::NV12.to_string(), "NV12");
}

#[test]
fn frame_formats_non_empty() {
    assert!(!frame_formats().is_empty());
}

#[test]
fn color_frame_formats_subset_of_all() {
    let all = frame_formats();
    for fmt in color_frame_formats() {
        assert!(all.contains(fmt), "{fmt:?} not in frame_formats()");
    }
}

#[test]
fn color_frame_formats_excludes_gray() {
    // The semantic contract of `color_frame_formats()` is "every
    // chroma-bearing format" — `GRAY` must be excluded because it
    // drives format-filter branches in
    // `RequestedFormat::fulfill`. A future refactor that
    // accidentally adds `GRAY` to the list (e.g. copy-pasting from
    // `frame_formats()`) would silently route GRAY cameras through
    // a color decode pipeline. The existing
    // `color_frame_formats_subset_of_all` test only verifies the
    // forward direction (every entry is in `frame_formats`); it
    // says nothing about which entries must NOT appear.
    assert!(
        !color_frame_formats().contains(&FrameFormat::GRAY),
        "color_frame_formats() must not contain GRAY"
    );
}

#[test]
fn color_frame_formats_includes_every_non_gray_format() {
    // The reverse contract: every non-GRAY entry in
    // `frame_formats()` must appear in `color_frame_formats()`.
    // Pins the bijection so a refactor that drops a chroma format
    // (e.g. removing NV12 from one list but not the other) is
    // caught immediately.
    for fmt in frame_formats() {
        if *fmt == FrameFormat::GRAY {
            continue;
        }
        assert!(
            color_frame_formats().contains(fmt),
            "{fmt:?} is non-GRAY but missing from color_frame_formats()"
        );
    }
}

#[test]
fn camera_format_new() {
    let res = Resolution::new(1920, 1080);
    let fmt = CameraFormat::new(res, FrameFormat::MJPEG, 30);
    assert_eq!(fmt.resolution(), res);
    assert_eq!(fmt.format(), FrameFormat::MJPEG);
    assert_eq!(fmt.frame_rate(), 30);
    assert_eq!(fmt.width(), 1920);
    assert_eq!(fmt.height(), 1080);
}

#[test]
fn camera_format_new_from() {
    let fmt = CameraFormat::new_from(640, 480, FrameFormat::YUYV, 15);
    assert_eq!(fmt.width(), 640);
    assert_eq!(fmt.height(), 480);
    assert_eq!(fmt.format(), FrameFormat::YUYV);
    assert_eq!(fmt.frame_rate(), 15);
    // `new_from` constructs `Resolution` directly via field literals
    // (`Resolution { width_x: res_x, height_y: res_y }`,
    // `nokhwa-core/src/types.rs:617-621`) rather than via
    // `Resolution::new`. The width/height accessor pins above
    // already catch x/y swaps, but they don't catch a regression
    // where the inner `Resolution` is built with a private
    // invariant field that `Resolution::new` would set but
    // `new_from`'s field-literal construction would skip — the
    // resulting `Resolution` would compare unequal to one made via
    // `Resolution::new(640, 480)` even though `width()`/`height()`
    // round-trip the same. Pin the structural equality contract.
    assert_eq!(fmt.resolution(), Resolution::new(640, 480));
}

#[test]
fn camera_format_default() {
    let fmt = CameraFormat::default();
    assert_eq!(fmt.width(), 640);
    assert_eq!(fmt.height(), 480);
    assert_eq!(fmt.frame_rate(), 30);
    assert_eq!(fmt.format(), FrameFormat::MJPEG);
}

#[test]
fn camera_format_setters() {
    let mut fmt = CameraFormat::default();
    fmt.set_resolution(Resolution::new(1280, 720));
    fmt.set_frame_rate(60);
    fmt.set_format(FrameFormat::NV12);
    assert_eq!(fmt.resolution(), Resolution::new(1280, 720));
    assert_eq!(fmt.frame_rate(), 60);
    assert_eq!(fmt.format(), FrameFormat::NV12);
}

#[test]
fn camera_format_display() {
    let fmt = CameraFormat::default();
    let display = format!("{fmt}");
    assert!(!display.is_empty());
}

/// Pin the exact `CameraFormat::Display` rendering. The format string
/// `"{resolution}@{fps}FPS, {format} Format"` is used by
/// `RequestedFormat::Display` (which embeds it via `{self:?}` →
/// `Debug`, but the rendered request lands in user-visible error
/// messages like `"Cannot fulfill request: …"`) and is also a
/// natural log-line shape. A regression that flips the order or
/// drops the `FPS`/`Format` literals would silently break dashboards
/// that grep for these tokens.
#[test]
fn camera_format_display_renders_resolution_at_fps_then_format() {
    let fmt = CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30);
    assert_eq!(format!("{fmt}"), "1920x1080@30FPS, MJPEG Format");
    let yuyv = CameraFormat::new_from(640, 480, FrameFormat::YUYV, 60);
    assert_eq!(format!("{yuyv}"), "640x480@60FPS, YUYV Format");
}

/// Pin the exact `CameraInfo::Display` rendering. The format string
/// `"Name: {n}, Description: {d}, Extra: {m}, Index: {i}"` is the
/// canonical shape backends use when logging device discovery, and
/// user code may parse it (e.g. a CLI that lists cameras with
/// `format!("{info}")`). A regression that re-orders the fields or
/// renames the labels would break downstream parsers.
#[test]
fn camera_info_display_renders_name_description_extra_index() {
    let info = CameraInfo::new(
        "Logitech BRIO",
        "USB Video Class Device",
        "/dev/video0",
        CameraIndex::Index(2),
    );
    assert_eq!(
        format!("{info}"),
        "Name: Logitech BRIO, Description: USB Video Class Device, Extra: /dev/video0, Index: 2"
    );
    let url_info = CameraInfo::new(
        "RTSP Stream",
        "GStreamer URL Source",
        "",
        CameraIndex::String("rtsp://example.com/cam".to_string()),
    );
    assert_eq!(
        format!("{url_info}"),
        "Name: RTSP Stream, Description: GStreamer URL Source, Extra: , Index: rtsp://example.com/cam"
    );
}

#[test]
fn camera_format_ordering_is_lexicographic_resolution_format_framerate() {
    let small_low_fps = CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30);
    let small_high_fps = CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 60);
    let small_yuyv = CameraFormat::new_from(640, 480, FrameFormat::YUYV, 30);
    let big = CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30);

    assert!(small_low_fps < small_high_fps);
    assert_eq!(
        small_low_fps.cmp(&small_yuyv),
        FrameFormat::MJPEG.cmp(&FrameFormat::YUYV)
    );
    assert!(small_low_fps < big);
    assert!(small_high_fps < big);
}

#[test]
fn camera_index_from_u32() {
    let idx = CameraIndex::Index(0u32);
    assert!(idx.is_index());
    assert!(!idx.is_string());
    assert_eq!(idx.as_index().unwrap(), 0);
}

#[test]
fn camera_index_string() {
    let idx = CameraIndex::String("/dev/video0".to_string());
    assert!(idx.is_string());
    assert!(!idx.is_index());
    assert_eq!(idx.as_string(), "/dev/video0");
}

#[test]
fn camera_index_default_is_index_zero() {
    let idx = CameraIndex::default();
    assert!(idx.is_index());
    assert_eq!(idx.as_index().unwrap(), 0);
    // Direct structural equality. The above checks would still pass
    // if `Default::default()` returned a *different* `Index(_)` whose
    // numeric value happened to coerce to 0 via some hypothetical
    // future `as_index` path; pin the exact variant + payload.
    assert_eq!(idx, CameraIndex::Index(0));
}

// `CameraIndex` derives `PartialEq` (`nokhwa-core/src/types.rs:304`),
// which is *structural* — `Index(0)` and `String("0")` are distinct
// even though `as_index()` resolves both to the same numeric value.
// No existing test pins this. A plausible "fix" replacing the derive
// with a hand-written impl that delegates to `as_index()` (rationale:
// "the dual-form resolution logic should be transitive across
// variants") would silently make `Index(3) == String("3")`,
// breaking camera deduplication in device enumeration where
// `Vec<CameraInfo>::contains` is used to filter duplicates between
// numeric V4L indices and string-form GStreamer URIs that happen to
// stringify to the same digit. Pin the structural contract.
#[test]
fn camera_index_partial_eq_is_structural_not_numeric() {
    // Cross-variant inequality even when as_index() agrees.
    assert_ne!(CameraIndex::Index(0), CameraIndex::String("0".into()));
    assert_ne!(CameraIndex::Index(42), CameraIndex::String("42".into()));
    // Same-variant equality (sanity).
    assert_eq!(CameraIndex::Index(0), CameraIndex::Index(0));
    assert_eq!(
        CameraIndex::String("rtsp://cam.local/stream".into()),
        CameraIndex::String("rtsp://cam.local/stream".into()),
    );
    // Same-variant inequality (sanity).
    assert_ne!(CameraIndex::Index(0), CameraIndex::Index(1));
    assert_ne!(
        CameraIndex::String("a".into()),
        CameraIndex::String("b".into()),
    );
}

#[test]
fn camera_info_getters_setters() {
    let mut info = CameraInfo::new(
        "Test Camera",
        "A test camera",
        "misc info",
        CameraIndex::Index(0),
    );
    assert_eq!(info.human_name(), "Test Camera");
    assert_eq!(info.description(), "A test camera");
    assert_eq!(info.misc(), "misc info");
    assert_eq!(info.index(), &CameraIndex::Index(0));

    info.set_human_name("New Name");
    assert_eq!(info.human_name(), "New Name");

    info.set_description("New desc");
    assert_eq!(info.description(), "New desc");

    info.set_misc("New misc");
    assert_eq!(info.misc(), "New misc");

    info.set_index(CameraIndex::Index(1));
    assert_eq!(info.index(), &CameraIndex::Index(1));

    // `set_index` is the only path the `CameraIndex::String(_)`
    // variant takes through `CameraInfo` after construction (e.g.
    // when a GStreamer backend rewrites a numeric V4L index to a
    // `rtsp://` URL after device discovery). The original test
    // only exercised `Index(_)`, so a regression that silently
    // dropped or coerced the `String` arm — e.g. `_ => self.index =
    // CameraIndex::Index(0)` left from a refactor — would slip
    // past. Round-trip the string arm too.
    info.set_index(CameraIndex::String("rtsp://cam.local/stream".into()));
    assert_eq!(
        info.index(),
        &CameraIndex::String("rtsp://cam.local/stream".into())
    );
}

#[test]
fn control_value_setter_accessors() {
    assert!(ControlValueSetter::None.as_none().is_some());
    assert_eq!(ControlValueSetter::Integer(42).as_integer(), Some(&42));
    assert_eq!(ControlValueSetter::Float(2.72).as_float(), Some(&2.72));
    assert_eq!(ControlValueSetter::Boolean(true).as_boolean(), Some(&true));
    assert_eq!(
        ControlValueSetter::String("hello".into()).as_str(),
        Some("hello")
    );
    assert_eq!(
        ControlValueSetter::Bytes(vec![1, 2, 3]).as_bytes(),
        Some([1u8, 2, 3].as_slice())
    );
}

#[test]
fn control_value_setter_wrong_type_returns_none() {
    assert!(ControlValueSetter::Integer(42).as_float().is_none());
    assert!(ControlValueSetter::Float(2.72).as_integer().is_none());
    assert!(ControlValueSetter::Boolean(true).as_str().is_none());
}

#[test]
fn camera_control_basic() {
    let control = CameraControl::new(
        KnownCameraControl::Brightness,
        "Brightness".to_string(),
        ControlValueDescription::Integer {
            value: 50,
            default: 50,
            step: 1,
        },
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    assert_eq!(control.control(), KnownCameraControl::Brightness);
    assert_eq!(control.name(), "Brightness");
    assert!(control.active());
    assert_eq!(control.flag(), &[KnownCameraControlFlag::Manual]);
}

#[test]
fn control_value_description_verify_setter() {
    let desc = ControlValueDescription::IntegerRange {
        min: 0,
        max: 100,
        value: 50,
        step: 1,
        default: 50,
    };
    assert!(desc.verify_setter(&ControlValueSetter::Integer(75)));
    assert!(!desc.verify_setter(&ControlValueSetter::Float(2.72)));
}

#[test]
fn closest_format_when_exact_resolution_unavailable() {
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 60),
    ];

    // Request 1280x720 which doesn't exist in the available formats
    let requested_fmt = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30);
    let req = RequestedFormat::with_formats(
        RequestedFormatType::Closest(requested_fmt),
        &[FrameFormat::MJPEG],
    );

    let result = req.fulfill(&available);
    assert!(
        result.is_some(),
        "Closest should return a format even when exact resolution is unavailable"
    );

    let result = result.unwrap();
    // 640x480 is the closest by Euclidean distance:
    // dist(1280,720 -> 640,480)   = sqrt(640^2 + 240^2) ≈ 683
    // dist(1280,720 -> 1920,1080) = sqrt(640^2 + 360^2) ≈ 734
    assert_eq!(result.resolution(), Resolution::new(640, 480));
    assert_eq!(result.format(), FrameFormat::MJPEG);
    assert_eq!(result.frame_rate(), 30);
}

// --- RequestedFormatType::fulfill() variant coverage ---

#[test]
fn fulfill_absolute_highest_resolution() {
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 60),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 60),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestResolution,
        &[FrameFormat::MJPEG],
    );
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.resolution(), Resolution::new(1920, 1080));
    // Among 1920x1080 formats, picks highest frame rate
    assert_eq!(result.frame_rate(), 60);
}

#[test]
fn fulfill_absolute_highest_framerate() {
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 120),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 120),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestFrameRate,
        &[FrameFormat::MJPEG],
    );
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.frame_rate(), 120);
    // Among 120fps formats, picks highest resolution
    assert_eq!(result.resolution(), Resolution::new(1920, 1080));
}

#[test]
fn fulfill_highest_resolution_at_given_resolution() {
    let available = vec![
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 60),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestResolution(Resolution::new(1280, 720)),
        &[FrameFormat::MJPEG],
    );
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.resolution(), Resolution::new(1280, 720));
    assert_eq!(result.frame_rate(), 60);
}

#[test]
fn fulfill_highest_resolution_no_match() {
    let available = vec![CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30)];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestResolution(Resolution::new(1920, 1080)),
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&available).is_none());
}

#[test]
fn fulfill_highest_resolution_returns_none_when_only_match_has_wrong_decoder() {
    // The requested resolution (1920x1080) IS present in the list, but only
    // with NV12 — and the caller only accepts MJPEG. The decoder filter at
    // types.rs:207 must drop the NV12 candidate before `max_by_key`, so the
    // `?` at types.rs:208 short-circuits to None.
    //
    // Distinct from `fulfill_highest_resolution_no_match`, which has no
    // candidate at the requested resolution at all. A regression that
    // forgot the `wanted_decoder.contains(...)` check on `HighestResolution`
    // would silently return the NV12 frame to an MJPEG-only caller.
    let available = vec![
        CameraFormat::new_from(1920, 1080, FrameFormat::NV12, 30),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestResolution(Resolution::new(1920, 1080)),
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&available).is_none());
}

#[test]
fn fulfill_highest_framerate_at_given_fps() {
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 60),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestFrameRate(30),
        &[FrameFormat::MJPEG],
    );
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.frame_rate(), 30);
    // Among 30fps formats, picks highest resolution
    assert_eq!(result.resolution(), Resolution::new(1280, 720));
}

#[test]
fn fulfill_highest_framerate_returns_none_when_only_match_has_wrong_decoder() {
    // Symmetric to the HighestResolution-decoder-filter test above:
    // the requested fps (60) IS present but only with NV12, and the
    // caller only accepts MJPEG. The decoder filter at types.rs:223
    // must drop the NV12 candidate before `max_by_key`, so the `?` at
    // types.rs:224 short-circuits to None.
    //
    // Distinct from `fulfill_highest_framerate_returns_none_when_no_candidate_at_fps`,
    // which has no candidate at the requested fps at all. A regression
    // that forgot the `wanted_decoder.contains(...)` check on
    // `HighestFrameRate` would silently return the NV12 frame to an
    // MJPEG-only caller.
    let available = vec![
        CameraFormat::new_from(1920, 1080, FrameFormat::NV12, 60),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestFrameRate(60),
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&available).is_none());
}

#[test]
fn fulfill_exact_match() {
    // Note: Exact variant does not check membership in the available list —
    // it only verifies the format matches the wanted decoder.
    let target = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30);
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
        target,
    ];
    let req =
        RequestedFormat::with_formats(RequestedFormatType::Exact(target), &[FrameFormat::MJPEG]);
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result, target);
}

#[test]
fn fulfill_exact_not_in_available_still_returns() {
    // Exact does not check membership — it only validates the decoder match.
    let target = CameraFormat::new_from(4096, 2160, FrameFormat::MJPEG, 120);
    let available = vec![CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30)];
    let req =
        RequestedFormat::with_formats(RequestedFormatType::Exact(target), &[FrameFormat::MJPEG]);
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result, target);
}

#[test]
fn fulfill_exact_wrong_decoder() {
    let target = CameraFormat::new_from(1280, 720, FrameFormat::NV12, 30);
    let available = vec![target];
    // Request MJPEG decoder but format is NV12
    let req =
        RequestedFormat::with_formats(RequestedFormatType::Exact(target), &[FrameFormat::MJPEG]);
    assert!(req.fulfill(&available).is_none());
}

#[test]
fn fulfill_none_returns_first_compatible() {
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::NV12, 30),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
    ];
    let req = RequestedFormat::with_formats(RequestedFormatType::None, &[FrameFormat::MJPEG]);
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.format(), FrameFormat::MJPEG);
}

#[test]
fn fulfill_none_no_compatible_format() {
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::NV12, 30),
        CameraFormat::new_from(640, 480, FrameFormat::YUYV, 30),
    ];
    let req = RequestedFormat::with_formats(RequestedFormatType::None, &[FrameFormat::GRAY]);
    assert!(req.fulfill(&available).is_none());
}

#[test]
fn fulfill_empty_format_list() {
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestResolution,
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&[]).is_none());
}

#[test]
fn fulfill_absolute_highest_framerate_returns_none_for_empty_format_list() {
    // `AbsoluteHighestFrameRate` filters by decoder set then runs
    // `max_by_key(|f| f.frame_rate())?`. On `&[]` the iterator is
    // empty and `?` short-circuits to `None`. Symmetric with
    // `fulfill_empty_format_list` (which only covers
    // `AbsoluteHighestResolution`); pinning this arm independently
    // catches a regression that adds a fallback to one variant
    // without the other.
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestFrameRate,
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&[]).is_none());
}

#[test]
fn fulfill_highest_framerate_returns_none_for_empty_format_list() {
    // `HighestFrameRate(fps)` filters by decoder set + frame_rate ==
    // fps then runs `max_by_key(|f| f.resolution())?`. Empty input
    // → empty filter → `?` short-circuits to `None`. Distinct from
    // `fulfill_highest_framerate_returns_none_when_no_candidate_at_fps`
    // (non-empty input but no match) because the filter chain is
    // skipped entirely with `&[]`.
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestFrameRate(30),
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&[]).is_none());
}

#[test]
fn fulfill_closest_picks_nearest_framerate() {
    let available = vec![
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 15),
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 60),
    ];
    let requested = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 25);
    let req = RequestedFormat::with_formats(
        RequestedFormatType::Closest(requested),
        &[FrameFormat::MJPEG],
    );
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.resolution(), Resolution::new(1280, 720));
    assert_eq!(result.frame_rate(), 30); // 30 is closest to 25
}

// `RequestedFormat::fulfill` is the cross-backend selection
// algorithm — every backend's `open()` ends up here. The no-match
// branches (filter excludes everything → `?` short-circuits to
// `None`) are easy to break with a future "let's fall back to
// something" tweak that silently reaches for the wrong format.
// Pin every short-circuit so a regression surfaces here, not on
// hardware.

#[test]
fn fulfill_closest_returns_none_when_format_not_in_decoder_set() {
    // `Closest` filters by `wanted_decoder.contains(&x.format())`
    // before computing distance. If no candidate survives the
    // filter, `resolution_map.first()?` short-circuits to `None`
    // — the contract is "Closest can fail when no candidate of
    // the requested kind exists", not "fall back to the nearest
    // wrong format".
    let available = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 60),
    ];
    let requested = CameraFormat::new_from(1280, 720, FrameFormat::YUYV, 30);
    let req = RequestedFormat::with_formats(
        RequestedFormatType::Closest(requested),
        &[FrameFormat::YUYV],
    );
    assert!(
        req.fulfill(&available).is_none(),
        "Closest must return None when no candidate matches the requested format"
    );
}

#[test]
fn fulfill_closest_returns_none_for_empty_format_list() {
    // Empty `all_formats` → empty `same_fmt_formats` → empty
    // `resolution_map` → `first()?` is `None`. Pin separately
    // from the format-mismatch case because the existing
    // `fulfill_empty_format_list` only covers
    // `AbsoluteHighestResolution`.
    let requested = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30);
    let req = RequestedFormat::with_formats(
        RequestedFormatType::Closest(requested),
        &[FrameFormat::MJPEG],
    );
    assert!(req.fulfill(&[]).is_none());
}

#[test]
fn fulfill_highest_framerate_returns_none_when_no_candidate_at_fps() {
    // `HighestFrameRate(fps)` filters to candidates whose
    // `frame_rate == fps` then `max_by_key(...)?`. If zero
    // candidates match, the `?` short-circuits to `None`. The
    // existing `fulfill_highest_resolution_no_match` pins this
    // for the resolution variant; this pins the symmetric
    // framerate variant.
    let available = vec![
        CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 60),
    ];
    let req = RequestedFormat::with_formats(
        RequestedFormatType::HighestFrameRate(120),
        &[FrameFormat::MJPEG],
    );
    assert!(
        req.fulfill(&available).is_none(),
        "HighestFrameRate must return None when no candidate matches the requested fps"
    );
}

#[test]
fn fulfill_closest_single_candidate_does_not_panic_and_picks_it() {
    // Sanity: with exactly one candidate of the right format,
    // `resolution_map.first()` and `framerate_map.first()` both
    // resolve to the only entry. Pinned because the algorithm
    // does `sort` + `dedup` + `first()` and a regression that
    // accidentally pops or skips the only entry would silently
    // return `None` for a perfectly answerable request.
    let only = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30);
    let requested = CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 60);
    let req = RequestedFormat::with_formats(
        RequestedFormatType::Closest(requested),
        &[FrameFormat::MJPEG],
    );
    let result = req
        .fulfill(&[only])
        .expect("Closest with single candidate must return that candidate");
    assert_eq!(result, only);
}

#[test]
fn fulfill_closest_distance_tie_picks_first_in_device_order() {
    // When two resolutions are equidistant from the requested target,
    // `RequestedFormat::fulfill` for `Closest`:
    //   1. computes `(dist_no_sqrt, res)` per candidate
    //   2. `sort_by_key(|a| a.0)` — Rust's stable sort, so equal keys
    //      retain their input order
    //   3. `dedup_by(|a, b| a.0.eq(&b.0))` — removes consecutive ties
    //      from the sorted slice, leaving the first of any tied run
    //   4. `first()` — picks the survivor
    //
    // Net contract: ties go to whichever resolution appears first in
    // `all_formats`. This is the only signal a backend has for
    // expressing preference between equidistant candidates, so a
    // refactor that swapped to `sort_unstable_by_key` (no stability
    // guarantee), or replaced `dedup_by` with a different pruning
    // step, would silently scramble selection on real cameras that
    // happen to advertise equidistant formats. Pinned with target
    // 100×100 vs candidates {50×100, 100×50} which both have
    // squared-distance 2500 — and a clearly-farther 200×200 control
    // (squared-distance 20000) so the test fails loudly if distance
    // computation itself breaks rather than just tie-breaking.
    let target = CameraFormat::new_from(100, 100, FrameFormat::MJPEG, 30);
    let req =
        RequestedFormat::with_formats(RequestedFormatType::Closest(target), &[FrameFormat::MJPEG]);

    let order_a = vec![
        CameraFormat::new_from(50, 100, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(100, 50, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(200, 200, FrameFormat::MJPEG, 30),
    ];
    let pick_a = req.fulfill(&order_a).unwrap();
    assert_eq!(
        pick_a.resolution(),
        Resolution::new(50, 100),
        "first equidistant candidate (by device order) must win",
    );

    let order_b = vec![
        CameraFormat::new_from(100, 50, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(50, 100, FrameFormat::MJPEG, 30),
        CameraFormat::new_from(200, 200, FrameFormat::MJPEG, 30),
    ];
    let pick_b = req.fulfill(&order_b).unwrap();
    assert_eq!(
        pick_b.resolution(),
        Resolution::new(100, 50),
        "swapping the input order must swap the winner — proves the \
         stable-sort + dedup-by-distance contract is what's pinned, \
         not an accidental Resolution-Ord tiebreaker",
    );
}

#[test]
fn fulfill_closest_framerate_tie_picks_first_in_device_order() {
    // Companion pin to `fulfill_closest_distance_tie_picks_first_in_device_order`.
    // Both arms of `Closest` use the same stable-sort-then-first
    // strategy for tie-breaking: resolution at
    // `nokhwa-core/src/types.rs:262-264`, framerate at lines 283-284.
    //
    // The framerate phase, unlike the resolution phase, does NOT call
    // `dedup_by` afterwards — relying entirely on Rust's stable
    // `sort_by_key` to keep input order for equal keys before
    // `.first()` picks the survivor.
    //
    // Net contract: when two framerates are equidistant from the
    // requested target (e.g. 15 and 45 around 30), the one whose
    // `CameraFormat` appears first in `all_formats` (and therefore
    // first in the post-resolution-filter `frame_rates` vector) wins.
    //
    // A refactor that swapped to `sort_unstable_by_key` would silently
    // scramble fps selection on cameras advertising symmetric fps
    // around the target (a real pattern: many cameras expose 15 and
    // 30, or 24 and 60, around a typical 30 target). The two
    // existing fps-related tests cover only unambiguous-winner cases:
    // `fulfill_closest_picks_nearest_framerate` uses {15, 30, 60} with
    // target 25 (clear winner: 30); the other Closest tests don't
    // probe ties at all.
    //
    // Test: target 30 fps with candidates {15, 45} — both have
    // distance 15. A clearly-farther 60 fps (distance 30) is included
    // as a sanity control so the test fails loudly if distance
    // computation itself breaks rather than just tie-breaking.
    let target = CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30);
    let req =
        RequestedFormat::with_formats(RequestedFormatType::Closest(target), &[FrameFormat::MJPEG]);

    let order_a = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 15),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 45),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 60),
    ];
    let pick_a = req.fulfill(&order_a).unwrap();
    assert_eq!(
        pick_a.frame_rate(),
        15,
        "first equidistant fps (by device order) must win",
    );

    let order_b = vec![
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 45),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 15),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 60),
    ];
    let pick_b = req.fulfill(&order_b).unwrap();
    assert_eq!(
        pick_b.frame_rate(),
        45,
        "swapping the input order must swap the winner — proves the \
         stable-sort contract on the framerate phase is what's \
         pinned, not an accidental fps-Ord tiebreaker",
    );
}

#[test]
fn fulfill_decoder_filter_applies_across_variants() {
    let available = vec![
        CameraFormat::new_from(1920, 1080, FrameFormat::NV12, 60),
        CameraFormat::new_from(640, 480, FrameFormat::YUYV, 30),
    ];
    // Only accept YUYV
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestResolution,
        &[FrameFormat::YUYV],
    );
    let result = req.fulfill(&available).unwrap();
    assert_eq!(result.format(), FrameFormat::YUYV);
    assert_eq!(result.resolution(), Resolution::new(640, 480));
}

// --- RequestedFormat::new::<F>() (typed constructor) coverage ---
//
// `RequestedFormat::new::<F: CaptureFormat>(...)` derives the FrameFormat
// constraint from `F::FRAME_FORMAT` via constant promotion. Every other
// `RequestedFormat` test goes through `with_formats`; nothing pinned the
// typed-constructor path. A regression that wired the wrong constant or
// broke the trait bound on `wanted_decoder` would slip past the rest of
// the suite.

#[test]
fn requested_format_new_constrains_to_marker_format_mjpeg() {
    use crate::format_types::Mjpeg;
    let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::AbsoluteHighestResolution);
    assert_eq!(
        req.requested_format_type(),
        RequestedFormatType::AbsoluteHighestResolution
    );
    let available = vec![
        CameraFormat::new_from(1920, 1080, FrameFormat::NV12, 30),
        CameraFormat::new_from(640, 480, FrameFormat::MJPEG, 30),
    ];
    let result = req
        .fulfill(&available)
        .expect("MJPEG marker must filter to the MJPEG entry");
    assert_eq!(
        result.format(),
        FrameFormat::MJPEG,
        "RequestedFormat::new::<Mjpeg> must reject non-MJPEG candidates"
    );
    assert_eq!(result.resolution(), Resolution::new(640, 480));
}

#[test]
fn requested_format_new_returns_none_when_no_compatible_format() {
    use crate::format_types::RawRgb;
    let req = RequestedFormat::new::<RawRgb>(RequestedFormatType::AbsoluteHighestResolution);
    let available = vec![CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30)];
    assert!(
        req.fulfill(&available).is_none(),
        "RequestedFormat::new::<RawRgb> must not accept an MJPEG-only device"
    );
}

#[test]
fn requested_format_new_yuyv_marker_filters_to_yuyv() {
    use crate::format_types::Yuyv;
    let req = RequestedFormat::new::<Yuyv>(RequestedFormatType::AbsoluteHighestFrameRate);
    let available = vec![
        CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 60),
        CameraFormat::new_from(1280, 720, FrameFormat::YUYV, 30),
        CameraFormat::new_from(640, 480, FrameFormat::YUYV, 60),
    ];
    let result = req.fulfill(&available).expect("YUYV marker must match");
    assert_eq!(result.format(), FrameFormat::YUYV);
    assert_eq!(
        result.frame_rate(),
        60,
        "AbsoluteHighestFrameRate over the YUYV-filtered subset is 60 fps"
    );
    assert_eq!(result.resolution(), Resolution::new(640, 480));
}

#[test]
fn requested_format_type_getter_round_trips_stored_variant() {
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestFrameRate,
        &[FrameFormat::YUYV],
    );
    assert_eq!(
        req.requested_format_type(),
        RequestedFormatType::AbsoluteHighestFrameRate
    );

    let exact = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 30);
    let req2 =
        RequestedFormat::with_formats(RequestedFormatType::Exact(exact), &[FrameFormat::MJPEG]);
    assert_eq!(
        req2.requested_format_type(),
        RequestedFormatType::Exact(exact)
    );
}

// --- ControlValueDescription::verify_setter() coverage ---

#[test]
fn verify_setter_none() {
    let desc = ControlValueDescription::None;
    assert!(desc.verify_setter(&ControlValueSetter::None));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(0)));
}

#[test]
fn verify_setter_integer_range_in_bounds() {
    let desc = ControlValueDescription::IntegerRange {
        min: 0,
        max: 100,
        value: 50,
        step: 1,
        default: 50,
    };
    assert!(desc.verify_setter(&ControlValueSetter::Integer(0)));
    assert!(desc.verify_setter(&ControlValueSetter::Integer(100)));
}

#[test]
fn verify_setter_integer_range_out_of_bounds() {
    let desc = ControlValueDescription::IntegerRange {
        min: 0,
        max: 100,
        value: 50,
        step: 1,
        default: 50,
    };
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(-1)));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(101)));
}

#[test]
fn verify_setter_integer_step_aligned_to_default() {
    // step > 1 with a default that is NOT a multiple of step. Valid setters
    // are those lying on the grid anchored at `default` (or `value`):
    // default + n*step. The alignment test must subtract the anchor, not add
    // it — adding accidentally inverts which values are accepted whenever the
    // anchor is itself off the step grid.
    let desc = ControlValueDescription::Integer {
        value: 3,
        default: 3,
        step: 5,
    };
    // 3, 8, 13, -2 ... are on the grid (3 + n*5).
    assert!(desc.verify_setter(&ControlValueSetter::Integer(3)));
    assert!(desc.verify_setter(&ControlValueSetter::Integer(8)));
    assert!(desc.verify_setter(&ControlValueSetter::Integer(-2)));
    // 7 is off the grid: (7 - 3) % 5 == 4 != 0.
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(7)));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(5)));
}

#[test]
fn verify_setter_integer_range_zero_step_always_valid() {
    // When step == 0, verify_setter returns true unconditionally —
    // even for mismatched types. This is the documented implementation behavior.
    let desc = ControlValueDescription::IntegerRange {
        min: 0,
        max: 100,
        value: 50,
        step: 0,
        default: 50,
    };
    assert!(desc.verify_setter(&ControlValueSetter::Integer(42)));
    assert!(desc.verify_setter(&ControlValueSetter::Float(2.72)));
}

#[test]
fn verify_setter_boolean() {
    let desc = ControlValueDescription::Boolean {
        value: true,
        default: false,
    };
    assert!(desc.verify_setter(&ControlValueSetter::Boolean(true)));
    assert!(desc.verify_setter(&ControlValueSetter::Boolean(false)));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_string() {
    let desc = ControlValueDescription::String {
        value: "current".to_string(),
        default: Some("default".to_string()),
    };
    assert!(desc.verify_setter(&ControlValueSetter::String("anything".into())));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(0)));
}

#[test]
fn verify_setter_bytes() {
    let desc = ControlValueDescription::Bytes {
        value: vec![1, 2, 3],
        default: vec![0],
    };
    assert!(desc.verify_setter(&ControlValueSetter::Bytes(vec![4, 5])));
    assert!(!desc.verify_setter(&ControlValueSetter::None));
}

#[test]
fn verify_setter_point_rejects_nan() {
    let desc = ControlValueDescription::Point {
        value: (0.0, 0.0),
        default: (0.0, 0.0),
    };
    assert!(desc.verify_setter(&ControlValueSetter::Point(1.0, 2.0)));
    assert!(!desc.verify_setter(&ControlValueSetter::Point(f64::NAN, 0.0)));
    assert!(!desc.verify_setter(&ControlValueSetter::Point(0.0, f64::NAN)));
    assert!(!desc.verify_setter(&ControlValueSetter::Point(f64::INFINITY, 0.0)));
}

#[test]
fn verify_setter_enum_checks_possible_values() {
    let desc = ControlValueDescription::Enum {
        value: 1,
        possible: vec![1, 2, 3],
        default: 1,
    };
    assert!(desc.verify_setter(&ControlValueSetter::EnumValue(1)));
    assert!(desc.verify_setter(&ControlValueSetter::EnumValue(3)));
    assert!(!desc.verify_setter(&ControlValueSetter::EnumValue(99)));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_float_range_in_bounds() {
    // Zero step bypasses all validation unconditionally.
    let desc_zero_step = ControlValueDescription::FloatRange {
        min: 0.0,
        max: 1.0,
        value: 0.5,
        step: 0.0,
        default: 0.5,
    };
    assert!(desc_zero_step.verify_setter(&ControlValueSetter::Float(0.75)));
    assert!(desc_zero_step.verify_setter(&ControlValueSetter::Float(999.0)));

    // Non-zero step exercises the actual range-checking logic.
    let desc_with_step = ControlValueDescription::FloatRange {
        min: 0.0,
        max: 1.0,
        value: 0.5,
        step: 0.5,
        default: 0.0,
    };
    assert!(desc_with_step.verify_setter(&ControlValueSetter::Float(0.5)));
    assert!(!desc_with_step.verify_setter(&ControlValueSetter::Float(2.0)));
    assert!(!desc_with_step.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_float_range_wrong_type() {
    let desc = ControlValueDescription::FloatRange {
        min: 0.0,
        max: 1.0,
        value: 0.5,
        step: 0.1,
        default: 0.5,
    };
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_float_range_out_of_bounds_and_non_finite() {
    // Companion to `verify_setter_float_range_in_bounds`: confirms
    // values below `min` are rejected, and NaN / infinity are
    // rejected via the step-alignment math (NaN % step == NaN,
    // which is `!= 0_f64`).
    let desc = ControlValueDescription::FloatRange {
        min: 0.0,
        max: 1.0,
        value: 0.5,
        step: 0.5,
        default: 0.0,
    };
    // Below min.
    assert!(!desc.verify_setter(&ControlValueSetter::Float(-0.5)));
    // NaN — step-alignment math yields NaN % step = NaN, which is
    // not equal to 0_f64.
    assert!(!desc.verify_setter(&ControlValueSetter::Float(f64::NAN)));
    // Infinity — same logic, infinity % step == NaN.
    assert!(!desc.verify_setter(&ControlValueSetter::Float(f64::INFINITY)));
    assert!(!desc.verify_setter(&ControlValueSetter::Float(f64::NEG_INFINITY)));
}

#[test]
fn verify_setter_float_non_finite_rejected() {
    // The unbounded `Float` variant — same step-alignment math —
    // must also reject NaN / infinity for consistency with the
    // bounded `FloatRange` and `Point` variants.
    let desc = ControlValueDescription::Float {
        value: 0.5,
        default: 0.5,
        step: 0.5,
    };
    assert!(!desc.verify_setter(&ControlValueSetter::Float(f64::NAN)));
    assert!(!desc.verify_setter(&ControlValueSetter::Float(f64::INFINITY)));
    assert!(!desc.verify_setter(&ControlValueSetter::Float(f64::NEG_INFINITY)));
}

#[test]
fn verify_setter_key_value_pair() {
    let desc = ControlValueDescription::KeyValuePair {
        key: 1,
        value: 2,
        default: (0, 0),
    };
    assert!(desc.verify_setter(&ControlValueSetter::KeyValue(10, 20)));
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_integer() {
    let desc = ControlValueDescription::Integer {
        value: 50,
        default: 50,
        step: 5,
    };
    // Alignment check uses OR logic: (i + default) % step == 0 || (i + value) % step == 0
    // Here value == default == 50, so both paths are equivalent: (5 + 50) % 5 == 0 → aligned
    assert!(desc.verify_setter(&ControlValueSetter::Integer(5)));
    // (3 + 50) % 5 == 3 → not aligned via either path
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(3)));
    assert!(!desc.verify_setter(&ControlValueSetter::Float(1.0)));

    // Test OR-logic: value and default differ so both alignment paths are distinct
    let desc_diff = ControlValueDescription::Integer {
        value: 7,
        default: 3,
        step: 5,
    };
    // i=2: (2+3)%5==0 → passes via default path (but (2+7)%5==4 → fails via value path)
    assert!(desc_diff.verify_setter(&ControlValueSetter::Integer(2)));
    // i=3: (3+3)%5==1 fails, (3+7)%5==0 → passes via value path
    assert!(desc_diff.verify_setter(&ControlValueSetter::Integer(3)));
    // i=1: (1+3)%5==4 fails, (1+7)%5==3 fails → both paths fail
    assert!(!desc_diff.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_integer_zero_step() {
    // When step == 0, verify_setter returns true unconditionally —
    // even for mismatched types (step==0 bypasses type checking).
    let desc = ControlValueDescription::Integer {
        value: 50,
        default: 50,
        step: 0,
    };
    assert!(desc.verify_setter(&ControlValueSetter::Integer(99)));
    assert!(desc.verify_setter(&ControlValueSetter::Float(1.0)));
}

#[test]
fn verify_setter_float() {
    // When step == 0.0, verify_setter returns true unconditionally.
    let desc_zero_step = ControlValueDescription::Float {
        value: 0.5,
        default: 0.5,
        step: 0.0,
    };
    assert!(desc_zero_step.verify_setter(&ControlValueSetter::Float(0.75)));
    assert!(desc_zero_step.verify_setter(&ControlValueSetter::Integer(1)));

    // With a non-zero step, test alignment and type rejection.
    let desc_with_step = ControlValueDescription::Float {
        value: 0.5,
        default: 0.5,
        step: 0.5,
    };
    // (1.0 - 0.5).abs() % 0.5 == 0.0 → aligned
    assert!(desc_with_step.verify_setter(&ControlValueSetter::Float(1.0)));
    // (0.75 - 0.5).abs() % 0.5 == 0.25 → not aligned
    assert!(!desc_with_step.verify_setter(&ControlValueSetter::Float(0.75)));
    assert!(!desc_with_step.verify_setter(&ControlValueSetter::Integer(1)));
}

#[test]
fn verify_setter_rgb() {
    // RGB verify_setter accepts any finite (non-NaN, non-infinite)
    // triple where each channel is within `0.0 ..= max`. Mirrors the
    // `value >= min && value <= max` shape used by IntegerRange /
    // FloatRange. See the inline comment in
    // `ControlValueDescription::verify_setter` for why this used to
    // be inverted.
    let desc = ControlValueDescription::RGB {
        value: (0.5, 0.5, 0.5),
        max: (1.0, 1.0, 1.0),
        default: (0.0, 0.0, 0.0),
    };
    // In-range values pass: at lower bound, at upper bound, in the middle.
    assert!(desc.verify_setter(&ControlValueSetter::RGB(0.0, 0.0, 0.0)));
    assert!(desc.verify_setter(&ControlValueSetter::RGB(0.5, 0.5, 0.5)));
    assert!(desc.verify_setter(&ControlValueSetter::RGB(1.0, 1.0, 1.0)));
    // Above max fails on any channel.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(2.0, 1.0, 1.0)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(1.0, 2.0, 1.0)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(1.0, 1.0, 2.0)));
    // Negative on any channel fails.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(-0.1, 0.5, 0.5)));
    // Non-finite (NaN, infinity) fails.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(f64::NAN, 0.5, 0.5)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(0.5, f64::INFINITY, 0.5)));
    // Wrong setter variant always fails.
    assert!(!desc.verify_setter(&ControlValueSetter::Integer(1)));
}

// `verify_setter` for RGB applies the upper-bound check
// independently per channel using `max.0` / `max.1` / `max.2`. The
// existing `verify_setter_rgb` test only uses the symmetric case
// `max = (1.0, 1.0, 1.0)`, so a regression that swaps `max.0` and
// `max.2` (or copies `max.0` to all three checks) would pass every
// existing assertion. Pin asymmetric maxima so per-channel index
// confusion surfaces here.

#[test]
fn verify_setter_rgb_asymmetric_per_channel_max() {
    // Each channel has a different ceiling; only triples that
    // respect each channel's own ceiling pass.
    let desc = ControlValueDescription::RGB {
        value: (50.0, 25.0, 0.5),
        max: (100.0, 50.0, 1.0),
        default: (0.0, 0.0, 0.0),
    };
    // In-range across all channels.
    assert!(desc.verify_setter(&ControlValueSetter::RGB(100.0, 50.0, 1.0)));
    assert!(desc.verify_setter(&ControlValueSetter::RGB(50.0, 25.0, 0.5)));

    // R at G's ceiling but G at R's ceiling — would pass if the
    // implementation used `max.0` for every channel. Must fail
    // because G=100 > max.1=50.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(50.0, 100.0, 0.5)));

    // B at G's ceiling — would pass if the implementation used
    // `max.1` for B. Must fail because B=50 > max.2=1.0.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(50.0, 25.0, 50.0)));

    // R at B's ceiling — would pass if the implementation used
    // `max.2` for R. Must fail because R=1.0 is fine for R, but
    // flip the check: R=200 with max.0=100 must fail.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(200.0, 25.0, 0.5)));
}

#[test]
fn verify_setter_rgb_zero_max_only_zero_passes() {
    // Edge case: a per-channel max of 0.0 collapses the valid set
    // to {0.0}. Useful if a backend reports a control as present
    // but unconfigurable. Pin so the inclusive-upper-bound check
    // (`x <= 0.0`) doesn't accidentally become exclusive (`x < 0.0`)
    // and reject every value including the only valid one.
    let desc = ControlValueDescription::RGB {
        value: (0.0, 0.0, 0.0),
        max: (0.0, 0.0, 0.0),
        default: (0.0, 0.0, 0.0),
    };
    assert!(desc.verify_setter(&ControlValueSetter::RGB(0.0, 0.0, 0.0)));
    // Anything positive on any channel must fail.
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(0.001, 0.0, 0.0)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(0.0, 0.001, 0.0)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(0.0, 0.0, 0.001)));
}

#[test]
fn verify_setter_rgb_negative_infinity_fails_per_channel() {
    // `f64::NEG_INFINITY` is non-finite *and* below zero — both
    // legs of `is_finite() && x >= 0.0` must reject it. Pin per
    // channel so the check isn't accidentally short-circuited
    // for one position.
    let desc = ControlValueDescription::RGB {
        value: (0.5, 0.5, 0.5),
        max: (1.0, 1.0, 1.0),
        default: (0.0, 0.0, 0.0),
    };
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(f64::NEG_INFINITY, 0.5, 0.5)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(0.5, f64::NEG_INFINITY, 0.5)));
    assert!(!desc.verify_setter(&ControlValueSetter::RGB(0.5, 0.5, f64::NEG_INFINITY)));
}

// --- CameraControl value round-trip tests ---

#[test]
fn control_value_roundtrip_integer() {
    let desc = ControlValueDescription::Integer {
        value: 42,
        default: 0,
        step: 1,
    };
    let control = CameraControl::new(
        KnownCameraControl::Brightness,
        "Brightness".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Integer(42));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_integer_range() {
    let desc = ControlValueDescription::IntegerRange {
        min: -100,
        max: 100,
        value: 75,
        step: 5,
        default: 0,
    };
    let control = CameraControl::new(
        KnownCameraControl::Contrast,
        "Contrast".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Integer(75));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_float() {
    let desc = ControlValueDescription::Float {
        value: 1.5,
        default: 1.0,
        step: 0.5,
    };
    let control = CameraControl::new(
        KnownCameraControl::Gamma,
        "Gamma".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Float(1.5));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_float_range() {
    let desc = ControlValueDescription::FloatRange {
        min: 0.0,
        max: 10.0,
        value: 5.0,
        step: 0.5,
        default: 1.0,
    };
    let control = CameraControl::new(
        KnownCameraControl::Exposure,
        "Exposure".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Automatic],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Float(5.0));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_boolean() {
    let desc = ControlValueDescription::Boolean {
        value: true,
        default: false,
    };
    let control = CameraControl::new(
        KnownCameraControl::BacklightComp,
        "BacklightComp".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Boolean(true));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_string() {
    let desc = ControlValueDescription::String {
        value: "hello".to_string(),
        default: Some("world".to_string()),
    };
    let control = CameraControl::new(
        KnownCameraControl::Other(999),
        "CustomString".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::ReadOnly],
        false,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::String("hello".to_string()));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_bytes() {
    let desc = ControlValueDescription::Bytes {
        value: vec![0xDE, 0xAD],
        default: vec![0x00],
    };
    let control = CameraControl::new(
        KnownCameraControl::Other(1000),
        "CustomBytes".to_string(),
        desc.clone(),
        vec![],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Bytes(vec![0xDE, 0xAD]));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_key_value() {
    let desc = ControlValueDescription::KeyValuePair {
        key: 10,
        value: 20,
        default: (0, 0),
    };
    let control = CameraControl::new(
        KnownCameraControl::Other(2000),
        "KVPair".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::WriteOnly],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::KeyValue(10, 20));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_point() {
    let desc = ControlValueDescription::Point {
        value: (1.5, 2.5),
        default: (0.0, 0.0),
    };
    let control = CameraControl::new(
        KnownCameraControl::Pan,
        "Pan".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Continuous],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::Point(1.5, 2.5));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_enum() {
    let desc = ControlValueDescription::Enum {
        value: 2,
        possible: vec![1, 2, 3, 4],
        default: 1,
    };
    let control = CameraControl::new(
        KnownCameraControl::WhiteBalance,
        "WhiteBalance".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::EnumValue(2));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_rgb() {
    // The current value lies strictly inside the [0, max] cube on each
    // channel — verify_setter should accept it. (Previously this test
    // relied on the inverted `>= max` predicate; see
    // `verify_setter_rgb` for the full coverage.)
    let desc = ControlValueDescription::RGB {
        value: (0.25, 0.5, 0.75),
        max: (1.0, 1.0, 1.0),
        default: (0.0, 0.0, 0.0),
    };
    let control = CameraControl::new(
        KnownCameraControl::Other(3000),
        "RGBControl".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Volatile],
        true,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::RGB(0.25, 0.5, 0.75));
    assert!(desc.verify_setter(&setter));
}

#[test]
fn control_value_roundtrip_none() {
    let desc = ControlValueDescription::None;
    let control = CameraControl::new(
        KnownCameraControl::Other(0),
        "NoneCtrl".to_string(),
        desc.clone(),
        vec![KnownCameraControlFlag::Disabled],
        false,
    );
    let setter = control.value();
    assert_eq!(setter, ControlValueSetter::None);
    assert!(desc.verify_setter(&setter));
}

#[test]
fn known_camera_control_all_variants_display_non_empty() {
    let controls = [
        KnownCameraControl::Brightness,
        KnownCameraControl::Contrast,
        KnownCameraControl::Hue,
        KnownCameraControl::Saturation,
        KnownCameraControl::Sharpness,
        KnownCameraControl::Gamma,
        KnownCameraControl::WhiteBalance,
        KnownCameraControl::BacklightComp,
        KnownCameraControl::Gain,
        KnownCameraControl::Pan,
        KnownCameraControl::Tilt,
        KnownCameraControl::Zoom,
        KnownCameraControl::Exposure,
        KnownCameraControl::Iris,
        KnownCameraControl::Focus,
        KnownCameraControl::Other(12345),
    ];
    for ctrl in &controls {
        // Verify Display doesn't panic and produces non-empty output
        let display = format!("{ctrl}");
        assert!(!display.is_empty(), "Display for {ctrl:?} was empty");
    }
}

#[test]
fn camera_control_set_active_toggle() {
    let mut control = CameraControl::new(
        KnownCameraControl::Focus,
        "Focus".to_string(),
        ControlValueDescription::Integer {
            value: 50,
            default: 50,
            step: 1,
        },
        vec![
            KnownCameraControlFlag::Manual,
            KnownCameraControlFlag::Automatic,
        ],
        true,
    );
    assert!(control.active());
    control.set_active(false);
    assert!(!control.active());
    control.set_active(true);
    assert!(control.active());
}

#[test]
fn camera_control_multiple_flags_preserved() {
    let flags = vec![
        KnownCameraControlFlag::Manual,
        KnownCameraControlFlag::Volatile,
        KnownCameraControlFlag::Continuous,
    ];
    let control = CameraControl::new(
        KnownCameraControl::Zoom,
        "Zoom".to_string(),
        ControlValueDescription::IntegerRange {
            min: 1,
            max: 10,
            value: 5,
            step: 1,
            default: 1,
        },
        flags.clone(),
        true,
    );
    assert_eq!(control.flag(), &flags);
}

#[test]
fn control_value_description_value_extraction_all_variants() {
    // Verify that .value() on every ControlValueDescription variant returns
    // a ControlValueSetter that matches the stored value.
    let cases: Vec<(ControlValueDescription, ControlValueSetter)> = vec![
        (ControlValueDescription::None, ControlValueSetter::None),
        (
            ControlValueDescription::Integer {
                value: -7,
                default: 0,
                step: 1,
            },
            ControlValueSetter::Integer(-7),
        ),
        (
            ControlValueDescription::IntegerRange {
                min: 0,
                max: 255,
                value: 128,
                step: 1,
                default: 0,
            },
            ControlValueSetter::Integer(128),
        ),
        (
            ControlValueDescription::Float {
                value: 3.14,
                default: 0.0,
                step: 0.01,
            },
            ControlValueSetter::Float(3.14),
        ),
        (
            ControlValueDescription::FloatRange {
                min: -1.0,
                max: 1.0,
                value: 0.5,
                step: 0.1,
                default: 0.0,
            },
            ControlValueSetter::Float(0.5),
        ),
        (
            ControlValueDescription::Boolean {
                value: false,
                default: true,
            },
            ControlValueSetter::Boolean(false),
        ),
        (
            ControlValueDescription::String {
                value: "test".to_string(),
                default: None,
            },
            ControlValueSetter::String("test".to_string()),
        ),
        (
            ControlValueDescription::Bytes {
                value: vec![1, 2, 3],
                default: vec![],
            },
            ControlValueSetter::Bytes(vec![1, 2, 3]),
        ),
        (
            ControlValueDescription::KeyValuePair {
                key: 42,
                value: 84,
                default: (0, 0),
            },
            ControlValueSetter::KeyValue(42, 84),
        ),
        (
            ControlValueDescription::Point {
                value: (9.0, 10.0),
                default: (0.0, 0.0),
            },
            ControlValueSetter::Point(9.0, 10.0),
        ),
        (
            ControlValueDescription::Enum {
                value: 3,
                possible: vec![1, 2, 3],
                default: 1,
            },
            ControlValueSetter::EnumValue(3),
        ),
        (
            ControlValueDescription::RGB {
                value: (0.1, 0.2, 0.3),
                max: (1.0, 1.0, 1.0),
                default: (0.0, 0.0, 0.0),
            },
            ControlValueSetter::RGB(0.1, 0.2, 0.3),
        ),
    ];

    for (desc, expected_setter) in &cases {
        assert_eq!(
            desc.value(),
            *expected_setter,
            "value() mismatch for {desc:?}"
        );
    }
}

// --- FrameFormat parse edge cases ---

#[test]
fn frame_format_parse_invalid_returns_error() {
    // `FrameFormat::FromStr` (`types.rs:418-435`) emits
    // `StructureError { structure: "FrameFormat", error: "No match for {s}" }`
    // on every reject path — including the empty-string and
    // wrong-case branches. The previous `is_err()` triplet would
    // pass even if the variant changed (e.g. to `GeneralError`)
    // or the prefix dropped (`{s}` vs `"No match for {s}"`),
    // both of which would silently break log scrapers and any
    // caller pattern-matching the `StructureError` variant.
    for token in ["INVALID", "", "mjpeg"] {
        let err = token
            .parse::<FrameFormat>()
            .expect_err("invalid token must err");
        match err {
            NokhwaError::StructureError { structure, error } => {
                assert_eq!(structure, "FrameFormat");
                assert_eq!(error, format!("No match for {token}"));
            },
            other => panic!("expected StructureError for {token:?}, got {other:?}"),
        }
    }
}

#[test]
fn frame_format_rawbgr_display_parse() {
    let fmt = FrameFormat::RAWBGR;
    let s = format!("{fmt}");
    assert_eq!(s, "RAWBGR");
    let parsed: FrameFormat = s.parse().unwrap();
    assert_eq!(parsed, FrameFormat::RAWBGR);
}

// --- CameraIndex additional coverage ---

#[test]
fn camera_index_as_index_returns_err_for_string() {
    // `CameraIndex::as_index` (`types.rs:315-322`) maps `ParseIntError`
    // through `NokhwaError::general`, producing `GeneralError` with no
    // backend tag and a message that's the verbatim libstd
    // `ParseIntError::Display`. Pin both — drift on either would slip
    // past `is_err()` but break any caller pattern-matching the
    // variant or scraping the message.
    let idx = CameraIndex::String("test".to_string());
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

// `CameraIndex::Display` delegates to `as_string()`, which emits the
// raw decimal for `Index(_)` and the inner `s.clone()` for
// `String(_)`. The previous test only asserted `s.contains('5')` for
// the `Index` variant and never exercised the `String` variant at
// all. A wrapping refactor like `format!("Index({i})")` /
// `format!("String({s})")` would silently break GStreamer URL
// dispatch error messages — `CameraIndex::String("rtsp://...")` is
// passed through `OpenedCamera`'s routing and surfaces in
// `OpenDeviceError`'s `{device}` slot. Pin both variants verbatim.
// Guards `nokhwa-core/src/types.rs:349-353`.
#[test]
fn camera_index_display_exact_format() {
    assert_eq!(CameraIndex::Index(5).to_string(), "5");
    assert_eq!(CameraIndex::Index(0).to_string(), "0");
    assert_eq!(
        CameraIndex::String("rtsp://cam.local/stream".to_string()).to_string(),
        "rtsp://cam.local/stream"
    );
    assert_eq!(
        CameraIndex::String("/dev/video0".to_string()).to_string(),
        "/dev/video0"
    );
}

#[test]
fn camera_index_as_index_parses_numeric_string() {
    // CameraIndex::String is the GStreamer URL escape hatch — but a
    // String("3") still represents an index, and as_index() must parse
    // it. This pins the dual-form contract: callers can store the
    // index-form as either variant and still resolve a u32.
    let idx = CameraIndex::String("3".to_string());
    assert_eq!(idx.as_index().unwrap(), 3);
}

#[test]
fn camera_index_as_string_stringifies_index() {
    // The Display impl delegates to as_string, so this also pins
    // the canonical decimal form (no leading zeros, no "0x" prefix)
    // that callers like nokhwa-bindings-* depend on when round-tripping
    // through device IDs.
    assert_eq!(CameraIndex::Index(0).as_string(), "0");
    assert_eq!(CameraIndex::Index(42).as_string(), "42");
}

#[test]
fn camera_index_try_from_u32_index() {
    let n: u32 = u32::try_from(CameraIndex::Index(7)).unwrap();
    assert_eq!(n, 7);
}

#[test]
fn camera_index_try_from_u32_numeric_string() {
    // The TryFrom path collapses to as_index(), so a String("9")
    // must convert successfully — the same contract callers rely on
    // when fishing a u32 out of a CameraIndex of either variant.
    let n: u32 = u32::try_from(CameraIndex::String("9".to_string())).unwrap();
    assert_eq!(n, 9);
}

#[test]
fn camera_index_try_from_u32_non_numeric_string_errs() {
    // `CameraIndex::as_index` (used by `TryFrom<CameraIndex> for u32` at
    // `types.rs:361`) maps `ParseIntError` through `NokhwaError::general`,
    // producing `GeneralError { backend: None, message: <ParseIntError> }`.
    // Pinning the variant + message guards against silent reroutes (e.g.
    // someone changing the helper to `StructureError`) and against drift in
    // the `ParseIntError` text we forward verbatim to callers.
    let err = u32::try_from(CameraIndex::String("/dev/video0".to_string())).unwrap_err();
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
fn camera_index_try_from_usize_index() {
    let n: usize = usize::try_from(CameraIndex::Index(11)).unwrap();
    assert_eq!(n, 11);
}

#[test]
fn camera_index_try_from_usize_numeric_string() {
    let n: usize = usize::try_from(CameraIndex::String("12".to_string())).unwrap();
    assert_eq!(n, 12);
}

#[test]
fn camera_index_try_from_usize_non_numeric_string_errs() {
    // `TryFrom<CameraIndex> for usize` at `types.rs:369` delegates to
    // `as_index` and casts on success, so the error path is identical to
    // the u32 case: `GeneralError` carrying the verbatim `ParseIntError`.
    let err = usize::try_from(CameraIndex::String("abc".to_string())).unwrap_err();
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

// --- ControlValueSetter additional accessors ---

#[test]
fn control_value_setter_key_value() {
    let setter = ControlValueSetter::KeyValue(10, 20);
    assert_eq!(setter.as_key_value(), Some((&10, &20)));
}

#[test]
fn control_value_setter_point() {
    let setter = ControlValueSetter::Point(1.5, 2.5);
    assert_eq!(setter.as_point(), Some((&1.5, &2.5)));
}

#[test]
fn control_value_setter_enum_value() {
    let setter = ControlValueSetter::EnumValue(42);
    assert_eq!(setter.as_enum(), Some(&42));
}

#[test]
fn control_value_setter_rgb() {
    let setter = ControlValueSetter::RGB(0.1, 0.2, 0.3);
    let (r, g, b) = setter.as_rgb().unwrap();
    assert!((r - 0.1).abs() < f64::EPSILON);
    assert!((g - 0.2).abs() < f64::EPSILON);
    assert!((b - 0.3).abs() < f64::EPSILON);
}

// ─── FrameFormat FourCC helpers ───

#[test]
fn frame_format_fourcc_roundtrip() {
    for &fmt in frame_formats() {
        assert_eq!(
            FrameFormat::from_fourcc(fmt.to_fourcc()),
            Some(fmt),
            "round-trip failed for {fmt:?}"
        );
    }
}

#[test]
fn frame_format_from_fourcc_unknown_returns_none() {
    assert_eq!(FrameFormat::from_fourcc("H264"), None);
    assert_eq!(FrameFormat::from_fourcc(""), None);
    assert_eq!(FrameFormat::from_fourcc("XXXX"), None);
}

#[test]
fn frame_format_to_fourcc_is_four_bytes() {
    for &fmt in frame_formats() {
        assert_eq!(
            fmt.to_fourcc().len(),
            4,
            "FourCC for {fmt:?} is not 4 bytes"
        );
    }
}

// Pin the *named* literals on both sides of the FourCC table so that
// `frame_format_fourcc_roundtrip` cannot be silently green when two
// entries are swapped — e.g. `RGB3 ↔ BGR3` would still round-trip but
// would mis-identify raw frames coming off a backend.
// Guards `nokhwa-core/src/types.rs:443-466`.
#[test]
fn frame_format_from_fourcc_named_literals() {
    assert_eq!(FrameFormat::from_fourcc("MJPG"), Some(FrameFormat::MJPEG));
    assert_eq!(FrameFormat::from_fourcc("YUYV"), Some(FrameFormat::YUYV));
    assert_eq!(FrameFormat::from_fourcc("GRAY"), Some(FrameFormat::GRAY));
    assert_eq!(FrameFormat::from_fourcc("RGB3"), Some(FrameFormat::RAWRGB));
    assert_eq!(FrameFormat::from_fourcc("BGR3"), Some(FrameFormat::RAWBGR));
    assert_eq!(FrameFormat::from_fourcc("NV12"), Some(FrameFormat::NV12));
}

#[test]
fn frame_format_to_fourcc_named_literals() {
    assert_eq!(FrameFormat::MJPEG.to_fourcc(), "MJPG");
    assert_eq!(FrameFormat::YUYV.to_fourcc(), "YUYV");
    assert_eq!(FrameFormat::GRAY.to_fourcc(), "GRAY");
    assert_eq!(FrameFormat::RAWRGB.to_fourcc(), "RGB3");
    assert_eq!(FrameFormat::RAWBGR.to_fourcc(), "BGR3");
    assert_eq!(FrameFormat::NV12.to_fourcc(), "NV12");
}

// ─── FrameFormat::decoded_pixel_byte_width ───

#[test]
fn frame_format_decoded_pixel_byte_width_gray_is_one() {
    assert_eq!(FrameFormat::GRAY.decoded_pixel_byte_width(), 1);
}

#[test]
fn frame_format_decoded_pixel_byte_width_color_formats_are_three() {
    for &fmt in &[
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
        FrameFormat::NV12,
    ] {
        assert_eq!(
            fmt.decoded_pixel_byte_width(),
            3,
            "{fmt:?} expected 3 bytes/pixel after decode"
        );
    }
}

#[test]
fn frame_format_decoded_pixel_byte_width_total_coverage() {
    // Catches a future variant added without being classified.
    for &fmt in frame_formats() {
        let bpp = fmt.decoded_pixel_byte_width();
        assert!(
            bpp == 1 || bpp == 3,
            "{fmt:?} returned unexpected bpp {bpp} — must be 1 (gray) or 3 (color)"
        );
    }
}

// ─── FrameFormat FromStr ───
//
// `from_fourcc` parses 4-byte FourCC tokens (e.g. "MJPG", "RGB3"), but
// `FromStr` parses the human-readable variant names (e.g. "MJPEG",
// "RAWRGB"). They are *distinct mappings* — confusing them is a
// common backend bug — so pin the FromStr table separately.

#[test]
fn frame_format_from_str_recognises_every_variant() {
    for &fmt in frame_formats() {
        let name = format!("{fmt:?}");
        let parsed: FrameFormat = name.parse().unwrap_or_else(|e| {
            panic!("FrameFormat::from_str({name:?}) should succeed but got: {e:?}")
        });
        assert_eq!(parsed, fmt, "FromStr({name:?}) round-trip mismatch");
    }
}

#[test]
fn frame_format_from_str_unknown_returns_structure_error() {
    // The `FromStr` impl at `nokhwa-core/src/types.rs:418-435` uses
    // `format!("No match for {s}")` for the error field. The previous
    // contains-only check would pass even if the prefix was dropped
    // (e.g. just `"H264"`) or the wording was reworded
    // (e.g. `"unknown FrameFormat: H264"`) — both of which would
    // silently change every downstream log line that includes this
    // diagnostic. Pin the verbatim format string.
    let err = "H264"
        .parse::<FrameFormat>()
        .expect_err("unknown should err");
    match err {
        NokhwaError::StructureError { structure, error } => {
            assert_eq!(structure, "FrameFormat");
            assert_eq!(error, "No match for H264");
        },
        other => panic!("expected StructureError, got {other:?}"),
    }
}

#[test]
fn frame_format_from_str_is_case_sensitive() {
    // "mjpeg" / "Mjpeg" (any non-uppercase) must NOT parse — pin this
    // so a future "be lenient" tweak is a deliberate, reviewed change
    // rather than silent surface drift. Pin the full variant so a
    // regression that swapped to `GeneralError` (or split the case-
    // sensitivity branch into a separate error type) is also caught.
    for token in ["mjpeg", "Mjpeg"] {
        let err = token
            .parse::<FrameFormat>()
            .expect_err("non-uppercase must err");
        match err {
            NokhwaError::StructureError { structure, error } => {
                assert_eq!(structure, "FrameFormat");
                assert_eq!(error, format!("No match for {token}"));
            },
            other => panic!("expected StructureError for {token:?}, got {other:?}"),
        }
    }
}

#[test]
fn frame_format_from_str_distinguished_from_fourcc() {
    // "MJPG" is a valid FourCC but NOT a valid FromStr token, and
    // "MJPEG" is a valid FromStr token but NOT a valid FourCC.
    // Pin the asymmetry to catch a future merge of the two tables.
    // For the FromStr-rejects "MJPG" pin the full variant so a regression
    // that quietly merges the two parse tables is caught at the variant
    // level (an empty `Ok(MJPEG)` would obviously fail; a reroute of
    // the error to a different variant would slip past `is_err()`).
    let err = "MJPG"
        .parse::<FrameFormat>()
        .expect_err("MJPG must NOT FromStr-parse");
    match err {
        NokhwaError::StructureError { structure, error } => {
            assert_eq!(structure, "FrameFormat");
            assert_eq!(error, "No match for MJPG");
        },
        other => panic!("expected StructureError, got {other:?}"),
    }
    assert_eq!(FrameFormat::from_fourcc("MJPEG"), None);
    assert_eq!(
        "MJPEG".parse::<FrameFormat>().ok(),
        Some(FrameFormat::MJPEG)
    );
    assert_eq!(FrameFormat::from_fourcc("MJPG"), Some(FrameFormat::MJPEG));
}

// ─── KnownCameraControl index helpers ───

#[test]
fn known_camera_control_index_roundtrip() {
    for ctrl in all_known_camera_controls() {
        let idx = ctrl.as_index().expect("standard control should have index");
        assert_eq!(
            KnownCameraControl::from_index(idx),
            Some(ctrl),
            "round-trip failed for {ctrl:?}"
        );
    }
}

#[test]
fn known_camera_control_other_has_no_index() {
    assert_eq!(KnownCameraControl::Other(42).as_index(), None);
}

#[test]
fn known_camera_control_from_index_out_of_range() {
    assert_eq!(KnownCameraControl::from_index(15), None);
    assert_eq!(KnownCameraControl::from_index(255), None);
}

#[test]
fn known_camera_control_standard_count_matches_all() {
    assert_eq!(
        KnownCameraControl::STANDARD_COUNT,
        all_known_camera_controls().len(),
        "STANDARD_COUNT and all_known_camera_controls() length must agree"
    );
}

// ─── KnownCameraControl platform-ID helpers ───

#[test]
fn known_camera_control_platform_id_roundtrip() {
    // Synthetic table: platform ID = canonical index * 100.
    let table: [u32; KnownCameraControl::STANDARD_COUNT] =
        core::array::from_fn(|i| (i as u32) * 100);

    for ctrl in all_known_camera_controls() {
        let pid = ctrl.to_platform_id(&table);
        let back = KnownCameraControl::from_platform_id(pid, &table);
        assert_eq!(back, ctrl, "round-trip failed for {ctrl:?}");
    }
}

#[test]
fn known_camera_control_from_platform_id_unknown_falls_back_to_other() {
    let table: [u32; KnownCameraControl::STANDARD_COUNT] =
        core::array::from_fn(|i| (i as u32) * 100);
    let unknown_id = 9999_u32;
    assert_eq!(
        KnownCameraControl::from_platform_id(unknown_id, &table),
        KnownCameraControl::Other(u128::from(unknown_id)),
    );
}

#[test]
fn known_camera_control_other_to_platform_id_truncates() {
    let table = [0_u32; KnownCameraControl::STANDARD_COUNT];
    let large_id: u128 = u128::from(u32::MAX) + 1;
    // Truncation: (u32::MAX + 1) as u32 == 0
    assert_eq!(
        KnownCameraControl::Other(large_id).to_platform_id(&table),
        0
    );
}

// `yuyv422_predicted_size` is a `pub` size predictor that callers
// pre-allocating the destination buffer for `buf_yuyv422_to_rgb` rely
// on. The arithmetic is `(input_size / 4) * (2 * 3)` for RGB and
// `(input_size / 4) * (2 * 4)` for RGBA — every 4-byte YUYV chunk
// produces 2 RGB or RGBA pixels. Pin the contract directly so a
// regression in either constant or the integer-division shape is
// caught.

#[test]
fn yuyv422_predicted_size_rgb_is_input_size_times_3_div_2() {
    // 4 input bytes (one chunk) → 2 RGB pixels = 6 bytes.
    assert_eq!(yuyv422_predicted_size(4, false), 6);
    // 1920x1080 YUYV is 1920*1080*2 = 4_147_200 bytes; RGB output is
    // 1920*1080*3 = 6_220_800 bytes. Confirm the formula scales.
    assert_eq!(
        yuyv422_predicted_size(1920 * 1080 * 2, false),
        1920 * 1080 * 3
    );
}

#[test]
fn yuyv422_predicted_size_rgba_is_input_size_times_2() {
    // 4 input bytes → 2 RGBA pixels = 8 bytes.
    assert_eq!(yuyv422_predicted_size(4, true), 8);
    // 1920x1080 YUYV → 1920*1080*4 RGBA bytes = exactly 2× input.
    assert_eq!(
        yuyv422_predicted_size(1920 * 1080 * 2, true),
        1920 * 1080 * 4
    );
}

#[test]
fn yuyv422_predicted_size_rounds_partial_chunks_down() {
    // Sub-chunk inputs round to zero rather than producing a partial
    // pixel — the destination buffer must hold an integral number of
    // RGB / RGBA pixels.
    assert_eq!(yuyv422_predicted_size(0, false), 0);
    assert_eq!(yuyv422_predicted_size(3, false), 0);
    assert_eq!(yuyv422_predicted_size(0, true), 0);
    assert_eq!(yuyv422_predicted_size(3, true), 0);
    // 5 bytes → 1 complete chunk + 1 leftover → 1 chunk × 2 pixels.
    assert_eq!(yuyv422_predicted_size(5, false), 6);
    assert_eq!(yuyv422_predicted_size(5, true), 8);
}

#[test]
fn yuyv422_predicted_size_matches_actual_yuyv422_to_rgb_output() {
    // `yuyv422_predicted_size` is what callers use to pre-size the
    // destination buffer for `yuyv422_to_rgb`; if the formulas drift
    // (e.g. someone changes one but not the other), every caller
    // gets either a buffer overrun or wasted memory. Cross-check by
    // running both functions on the same input and asserting the
    // predicted size equals the actual output length, for both RGB
    // and RGBA paths and for an input that exceeds the SIMD chunk
    // size so the partial-chunk + main-loop split is exercised.
    let data: Vec<u8> = (0..64u8).collect(); // 64 bytes = 16 YUYV chunks = 32 pixels
    let rgb = yuyv422_to_rgb(&data, false).expect("convert RGB must succeed");
    assert_eq!(yuyv422_predicted_size(data.len(), false), rgb.len());

    let rgba = yuyv422_to_rgb(&data, true).expect("convert RGBA must succeed");
    assert_eq!(yuyv422_predicted_size(data.len(), true), rgba.len());
}

// `yuyv444_to_rgb` is the per-pixel kernel for YCbCr-4:4:4 → RGB888
// (BT.601 video-range matrix). Pin the contract through the canonical
// reference points + the saturation/clamp behaviour so a typo in the
// integer coefficients (298 / 409 / 100 / 208 / 516 / 128 / 16)
// trips at the unit-test layer.

#[test]
fn yuyv444_to_rgb_video_range_black_is_zero() {
    // BT.601 video-range black: Y=16, Cb=Cr=128. Should round-trip
    // very close to (0, 0, 0) — the rounding constant `+128 >> 8`
    // makes this exact for the reference points.
    assert_eq!(yuyv444_to_rgb(16, 128, 128), [0, 0, 0]);
}

#[test]
fn yuyv444_to_rgb_video_range_white_is_max() {
    // BT.601 video-range white: Y=235, Cb=Cr=128. (235-16)*298 + 128
    // = 65430, >>8 = 255 exactly per channel (no clamp involvement).
    assert_eq!(yuyv444_to_rgb(235, 128, 128), [255, 255, 255]);
}

#[test]
fn yuyv444_to_rgb_grey_axis_has_equal_channels() {
    // On the Cb=Cr=128 grey axis, the chroma terms (`409*e`,
    // `-100*d-208*e`, `516*d`) all collapse to zero, leaving R=G=B
    // for every Y in [16, 235]. Sweep the legal range and confirm
    // monotonic non-decreasing intensity.
    let mut last = 0u8;
    for y in 16..=235 {
        let [r, g, b] = yuyv444_to_rgb(y, 128, 128);
        assert_eq!(r, g, "Y={y}: R/G drift on grey axis");
        assert_eq!(g, b, "Y={y}: G/B drift on grey axis");
        assert!(r >= last, "Y={y}: grey ramp not monotonic ({r} < {last})");
        last = r;
    }
    // Endpoints exact.
    assert_eq!(yuyv444_to_rgb(16, 128, 128)[0], 0);
    assert_eq!(yuyv444_to_rgb(235, 128, 128)[0], 255);
}

#[test]
fn yuyv444_to_rgb_clamps_extreme_inputs() {
    // Out-of-range YCbCr values (e.g. RGB-to-YUV synthesis with no
    // clipping) must not panic and must produce bytes in [0, 255].
    // Sweep a sparse grid that exercises each saturation arm:
    //   * Y=0 with Cb=Cr=128 → R/G/B underflow → 0
    //   * Y=255 with Cb=Cr=255 → R overflow → 255
    //   * Y=255 with Cb=255 Cr=0 → G/B asymmetric → still bounded
    for &(y, u, v) in &[
        (0_i32, 128, 128),
        (255, 128, 128),
        (255, 255, 255),
        (255, 0, 0),
        (255, 255, 0),
        (0, 0, 255),
    ] {
        let _ = yuyv444_to_rgb(y, u, v);
        // No assertion needed beyond "must not panic"; the function
        // returns `[u8; 3]` so the bytes are inherently in range.
    }
}

#[test]
fn yuyv444_to_rgba_matches_rgb_with_alpha_255() {
    // `yuyv444_to_rgba` is documented as `yuyv444_to_rgb + alpha=255`.
    // Pin that exact relationship across a small sample.
    for &(y, u, v) in &[
        (16, 128, 128),
        (235, 128, 128),
        (125, 128, 128),
        (90, 60, 200),
        (200, 200, 100),
    ] {
        let [r, g, b] = yuyv444_to_rgb(y, u, v);
        assert_eq!(yuyv444_to_rgba(y, u, v), [r, g, b, 255]);
    }
}

// ===== ApiBackend Display + equality =====

/// Each built-in `ApiBackend` variant must have a stable `Display`
/// rendering matching its variant name. `NokhwaError` variants embed
/// `ApiBackend` in `Display` output ("Could not initialize {backend}:
/// …", "Error (backend {b}): …", etc.) and downstream consumers
/// (logs, telemetry, error matchers) parse those strings — a rename
/// would silently break log-based dashboards. This pins the rendering
/// for every built-in variant + the `Custom` payload form.
#[test]
fn api_backend_display_renders_variant_name() {
    assert_eq!(ApiBackend::Auto.to_string(), "Auto");
    assert_eq!(ApiBackend::AVFoundation.to_string(), "AVFoundation");
    assert_eq!(ApiBackend::Video4Linux.to_string(), "Video4Linux");
    assert_eq!(ApiBackend::MediaFoundation.to_string(), "MediaFoundation");
    assert_eq!(ApiBackend::GStreamer.to_string(), "GStreamer");
    assert_eq!(ApiBackend::Browser.to_string(), "Browser");
    assert_eq!(
        ApiBackend::Custom("MyBackend".to_string()).to_string(),
        "Custom(\"MyBackend\")"
    );
}

/// `ApiBackend` is `Eq` / `Hash`, used as a key in CI workflow
/// dispatch (e.g. `tests/device_tests.rs::native_backend()` matches
/// against it). The seven built-in variants must all compare unequal
/// to one another, and `Custom(s)` must compare equal only when the
/// payload string matches. A regression that derived something other
/// than structural equality (e.g. an old hand-rolled `PartialEq` that
/// ignored `Custom`'s payload) would silently make every custom
/// backend collide.
#[test]
fn api_backend_equality_is_structural_and_pairwise_distinct() {
    let builtins = [
        ApiBackend::Auto,
        ApiBackend::AVFoundation,
        ApiBackend::Video4Linux,
        ApiBackend::MediaFoundation,
        ApiBackend::GStreamer,
        ApiBackend::Browser,
    ];
    for (i, a) in builtins.iter().enumerate() {
        assert_eq!(a, a, "{a:?} is not equal to itself");
        for (j, b) in builtins.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "{a:?} and {b:?} compare equal");
            }
        }
    }
    // Payload structure for Custom.
    assert_eq!(
        ApiBackend::Custom("a".to_string()),
        ApiBackend::Custom("a".to_string())
    );
    assert_ne!(
        ApiBackend::Custom("a".to_string()),
        ApiBackend::Custom("b".to_string())
    );
    // Custom must not collide with a built-in even when the string matches.
    assert_ne!(
        ApiBackend::Custom("Auto".to_string()),
        ApiBackend::Auto,
        "Custom(\"Auto\") must not collide with the built-in Auto variant"
    );
}

/// `ApiBackend` derives `Ord`. The derive places variants in
/// declaration order: `Auto < AVFoundation < Video4Linux <
/// MediaFoundation < GStreamer < Browser < Custom`. This test pins
/// that ordering — a re-ordering of the enum (e.g. an alphabetised
/// refactor) would silently break code that uses the type as a
/// `BTreeMap<ApiBackend, _>` key with stable iteration order.
#[test]
fn api_backend_derived_ord_is_declaration_order() {
    let mut variants = [
        ApiBackend::Browser,
        ApiBackend::AVFoundation,
        ApiBackend::Custom("z".to_string()),
        ApiBackend::Auto,
        ApiBackend::MediaFoundation,
        ApiBackend::Video4Linux,
        ApiBackend::GStreamer,
    ];
    variants.sort();
    assert_eq!(
        variants,
        [
            ApiBackend::Auto,
            ApiBackend::AVFoundation,
            ApiBackend::Video4Linux,
            ApiBackend::MediaFoundation,
            ApiBackend::GStreamer,
            ApiBackend::Browser,
            ApiBackend::Custom("z".to_string()),
        ]
    );
}

// ─── KnownCameraControl / KnownCameraControlFlag Display rendering ───
//
// All four Display impls in `types.rs` delegate to `{self:?}`. Pin the rendering
// so a future move away from Debug-piping is a deliberate, reviewed change.

// `KnownCameraControl::Display` delegates to `Debug`, but downstream
// (UI / log dashboards / `CameraControl::Display`'s embedded
// `"Control: {}, ..."` rendering) keys on the literal variant names.
// `known_camera_control_all_variants_display_non_empty` exercises
// every variant but only asserts `!is_empty()`, so a hand-written
// `Display` impl that renamed even one variant
// (e.g. `WhiteBalance` → `"WhiteBalanceTemp"`) would silently change
// the rendering surfaced in `NokhwaError::SetPropertyError`'s
// `{property}` slot. Pin every named variant verbatim plus a
// representative `Other(_)` rendering. Guards
// `nokhwa-core/src/types.rs:948-952`.
#[test]
fn known_camera_control_display_renders_variant_name() {
    assert_eq!(KnownCameraControl::Brightness.to_string(), "Brightness");
    assert_eq!(KnownCameraControl::Contrast.to_string(), "Contrast");
    assert_eq!(KnownCameraControl::Hue.to_string(), "Hue");
    assert_eq!(KnownCameraControl::Saturation.to_string(), "Saturation");
    assert_eq!(KnownCameraControl::Sharpness.to_string(), "Sharpness");
    assert_eq!(KnownCameraControl::Gamma.to_string(), "Gamma");
    assert_eq!(KnownCameraControl::WhiteBalance.to_string(), "WhiteBalance");
    assert_eq!(
        KnownCameraControl::BacklightComp.to_string(),
        "BacklightComp"
    );
    assert_eq!(KnownCameraControl::Gain.to_string(), "Gain");
    assert_eq!(KnownCameraControl::Pan.to_string(), "Pan");
    assert_eq!(KnownCameraControl::Tilt.to_string(), "Tilt");
    assert_eq!(KnownCameraControl::Zoom.to_string(), "Zoom");
    assert_eq!(KnownCameraControl::Exposure.to_string(), "Exposure");
    assert_eq!(KnownCameraControl::Iris.to_string(), "Iris");
    assert_eq!(KnownCameraControl::Focus.to_string(), "Focus");
    assert_eq!(KnownCameraControl::Other(42).to_string(), "Other(42)");
}

#[test]
fn known_camera_control_flag_display_renders_variant_name() {
    assert_eq!(KnownCameraControlFlag::Automatic.to_string(), "Automatic");
    assert_eq!(KnownCameraControlFlag::Manual.to_string(), "Manual");
    assert_eq!(KnownCameraControlFlag::Continuous.to_string(), "Continuous");
    assert_eq!(KnownCameraControlFlag::ReadOnly.to_string(), "ReadOnly");
    assert_eq!(KnownCameraControlFlag::WriteOnly.to_string(), "WriteOnly");
    assert_eq!(KnownCameraControlFlag::Volatile.to_string(), "Volatile");
    assert_eq!(KnownCameraControlFlag::Disabled.to_string(), "Disabled");
}

#[test]
fn requested_format_type_display_matches_debug() {
    let none = RequestedFormatType::None;
    assert_eq!(none.to_string(), format!("{none:?}"));
    assert_eq!(none.to_string(), "None");

    let abs_res = RequestedFormatType::AbsoluteHighestResolution;
    assert_eq!(abs_res.to_string(), "AbsoluteHighestResolution");

    let exact =
        RequestedFormatType::Exact(CameraFormat::new_from(1920, 1080, FrameFormat::MJPEG, 30));
    assert_eq!(exact.to_string(), format!("{exact:?}"));
}

#[test]
fn requested_format_type_display_matches_debug_all_remaining_variants() {
    // The existing test covers `None`, `AbsoluteHighestResolution`,
    // and `Exact`. The other four variants —
    // `AbsoluteHighestFrameRate`, `HighestResolution(_)`,
    // `HighestFrameRate(_)`, `Closest(_)` — were unpinned. The impl
    // is `write!(f, "{self:?}")` which looks safe today, but a
    // future hand-written `Display` replacing the `{:?}` delegation
    // would silently break downstream callers that embed these
    // strings into `NokhwaError::ReadFrameError` /
    // `SetPropertyError` payloads. Pin every payload-bearing
    // variant against its `Debug` rendering so the contract holds
    // for the whole enum.
    let abs_fps = RequestedFormatType::AbsoluteHighestFrameRate;
    assert_eq!(abs_fps.to_string(), format!("{abs_fps:?}"));
    assert_eq!(abs_fps.to_string(), "AbsoluteHighestFrameRate");

    let highest_res = RequestedFormatType::HighestResolution(Resolution::new(1920, 1080));
    assert_eq!(highest_res.to_string(), format!("{highest_res:?}"));

    let highest_fps = RequestedFormatType::HighestFrameRate(60);
    assert_eq!(highest_fps.to_string(), format!("{highest_fps:?}"));

    let closest =
        RequestedFormatType::Closest(CameraFormat::new_from(1280, 720, FrameFormat::YUYV, 30));
    assert_eq!(closest.to_string(), format!("{closest:?}"));
}

// `RequestedFormatType` carries a `#[default]` attribute on the
// `None` variant (`nokhwa-core/src/types.rs:34`). No test currently
// constructs `RequestedFormatType::default()` and asserts the chosen
// variant — every existing usage names the variant explicitly. A
// well-meaning refactor that moved `#[default]` to e.g.
// `AbsoluteHighestResolution` (under the rationale "users probably
// want highest-res by default") would silently change what
// `RequestedFormat::new::<F>(Default::default())` returns, breaking
// any downstream code that relied on the no-op None default to mean
// "negotiate later". Pin the chosen default variant.
#[test]
fn requested_format_type_default_is_none() {
    assert_eq!(
        RequestedFormatType::default(),
        RequestedFormatType::None,
        "#[default] must remain on the None variant"
    );
}

// `requested_format_type_display_matches_debug_all_remaining_variants`
// is symmetry-only for `HighestFrameRate(_)` — it asserts
// `to_string() == format!("{:?}")`, which would stay green if the
// `{self:?}` delegation in the `Display` impl was replaced with a
// hand-written renderer that emitted e.g. `"HighestFps(60)"` (both
// sides would change in lockstep and the equality holds). The
// `AbsoluteHighestFrameRate` arm already has an exact-string pin
// alongside its symmetry check; mirror that for the payload-bearing
// `HighestFrameRate(u32)` variant, since the bare integer payload
// (no embedded `Debug` of a complex struct) is stable enough to pin
// exactly without coupling the test to unrelated field renames.
#[test]
fn requested_format_type_display_highest_frame_rate_exact_format() {
    let highest_fps = RequestedFormatType::HighestFrameRate(30);
    assert_eq!(highest_fps.to_string(), "HighestFrameRate(30)");
    let highest_fps_zero = RequestedFormatType::HighestFrameRate(0);
    assert_eq!(highest_fps_zero.to_string(), "HighestFrameRate(0)");
}

#[test]
fn requested_format_display_matches_debug() {
    let req = RequestedFormat::with_formats(
        RequestedFormatType::AbsoluteHighestResolution,
        &[FrameFormat::MJPEG, FrameFormat::YUYV],
    );
    assert_eq!(req.to_string(), format!("{req:?}"));
}

// ─── ControlValueDescription Display rendering ───
//
// `ControlValueDescription::Display` produces the human-readable
// portion of `CameraControl` log lines and surfaces in
// `NokhwaError::SetPropertyError` payloads when a setter is rejected.
// Unlike the four Debug-piping Display impls above, each variant has
// a hand-written rendering that grew organically alongside the
// matching `verify_setter` arm — so a refactor of one variant is
// easy to land without noticing the Display drift. Pin each
// rendering shape so a regression that re-orders / renames fields
// triggers a unit-test failure rather than silently changing log
// output.

#[test]
fn control_value_description_display_none() {
    assert_eq!(ControlValueDescription::None.to_string(), "(None)");
}

#[test]
fn control_value_description_display_integer() {
    let desc = ControlValueDescription::Integer {
        value: 50,
        default: 0,
        step: 1,
    };
    assert_eq!(desc.to_string(), "(Current: 50, Default: 0, Step: 1)");
}

#[test]
fn control_value_description_display_integer_range() {
    let desc = ControlValueDescription::IntegerRange {
        min: -100,
        max: 100,
        value: 25,
        step: 5,
        default: 0,
    };
    assert_eq!(
        desc.to_string(),
        "(Current: 25, Default: 0, Step: 5, Range: (-100, 100))"
    );
}

#[test]
fn control_value_description_display_float() {
    let desc = ControlValueDescription::Float {
        value: 1.5,
        default: 0.0,
        step: 0.1,
    };
    assert_eq!(desc.to_string(), "(Current: 1.5, Default: 0, Step: 0.1)");
}

#[test]
fn control_value_description_display_float_range() {
    let desc = ControlValueDescription::FloatRange {
        min: 0.0,
        max: 1.0,
        value: 0.5,
        step: 0.01,
        default: 0.25,
    };
    assert_eq!(
        desc.to_string(),
        "(Current: 0.5, Default: 0.25, Step: 0.01, Range: (0, 1))"
    );
}

#[test]
fn control_value_description_display_boolean() {
    let desc = ControlValueDescription::Boolean {
        value: true,
        default: false,
    };
    assert_eq!(desc.to_string(), "(Current: true, Default: false)");
}

#[test]
fn control_value_description_display_string() {
    let desc = ControlValueDescription::String {
        value: "hello".to_string(),
        default: Some("world".to_string()),
    };
    assert_eq!(
        desc.to_string(),
        "(Current: hello, Default: Some(\"world\"))"
    );
}

// `ControlValueDescription::String` Displays its `default` field via
// `{:?}` on `Option<String>`, which emits asymmetric strings: `Some(_)`
// renders as `Some("value")` (with inner quotes), `None` renders as
// `None` (no quotes). The `Some` arm is pinned by
// `control_value_description_display_string` above; the `None` arm is
// not. A regression that replaced `{default:?}` with
// `{default.as_deref().unwrap_or("(unset)")}` would silently change
// the `None` rendering and break callers that key on `Default: None`
// when reading `NokhwaError::SetPropertyError` payloads. Guards
// `nokhwa-core/src/types.rs:1224-1226`.
#[test]
fn control_value_description_display_string_default_none() {
    let desc = ControlValueDescription::String {
        value: "v4l2_auto".to_string(),
        default: None,
    };
    assert_eq!(desc.to_string(), "(Current: v4l2_auto, Default: None)");
}

#[test]
fn control_value_description_display_key_value_pair() {
    let desc = ControlValueDescription::KeyValuePair {
        key: 1,
        value: 2,
        default: (3, 4),
    };
    assert_eq!(desc.to_string(), "Current: (1, 2), Default: (3, 4)");
}

// ControlValueDescription::Bytes formats both the current and default
// byte arrays with the lowercase-hex specifier (`{:x?}`). A refactor
// that drops the `x` (downgrading to `{:?}`) would silently emit
// decimal byte arrays in every Display rendering, which surfaces in
// `SetPropertyError` payloads, error messages, and `eprintln!`
// debugging — making opaque control blobs harder to read at the exact
// moment they're being investigated. Pin the `x?` formatter contract.
#[test]
fn control_value_description_display_bytes_uses_hex() {
    let desc = ControlValueDescription::Bytes {
        value: vec![0x01, 0xab, 0xff],
        default: vec![0x00, 0x10],
    };
    let s = desc.to_string();
    assert_eq!(
        s, "(Current: [1, ab, ff], Default: [0, 10])",
        "Bytes Display must use lowercase-hex (`{{:x?}}`); got {s}"
    );
}

#[test]
fn control_value_description_display_point() {
    let desc = ControlValueDescription::Point {
        value: (1.5, 2.5),
        default: (0.0, 0.0),
    };
    assert_eq!(desc.to_string(), "Current: (1.5, 2.5), Default: (0, 0)");
}

#[test]
fn control_value_description_display_enum() {
    let desc = ControlValueDescription::Enum {
        value: 1,
        possible: vec![0, 1, 2],
        default: 0,
    };
    assert_eq!(
        desc.to_string(),
        "Current: 1, Possible Values: [0, 1, 2], Default: 0"
    );
}

// `control_value_description_display_enum` exercises only single-digit
// values, so a regression that swapped `{possible:?}` for a hand-rolled
// `possible.iter().map(...).join(", ")` (dropping the outer `[]`
// brackets) would silently break the rendering, but only on lists with
// elements where Debug spacing actually matters. Pin two extra cases:
// (1) an empty `possible` list — confirms the bracket-only form `[]`,
// not `()` or empty-string; (2) two-digit values — confirms the
// `Vec<i64>` Debug spacing contract `[10, 20]` (comma-space separator).
// Together they guard `nokhwa-core/src/types.rs:1253-1257`'s reliance
// on `Vec<i64>::fmt::Debug` for the `Possible Values` field.
#[test]
fn control_value_description_display_enum_empty_possible_list() {
    let desc = ControlValueDescription::Enum {
        value: 0,
        possible: vec![],
        default: 0,
    };
    assert_eq!(
        desc.to_string(),
        "Current: 0, Possible Values: [], Default: 0"
    );
}

#[test]
fn control_value_description_display_enum_multi_digit_values() {
    let desc = ControlValueDescription::Enum {
        value: 10,
        possible: vec![10, 20],
        default: 10,
    };
    assert_eq!(
        desc.to_string(),
        "Current: 10, Possible Values: [10, 20], Default: 10"
    );
}

#[test]
fn control_value_description_display_rgb() {
    let desc = ControlValueDescription::RGB {
        value: (1.0, 2.0, 3.0),
        max: (255.0, 255.0, 255.0),
        default: (0.0, 0.0, 0.0),
    };
    assert_eq!(
        desc.to_string(),
        "Current: (1, 2, 3), Max: (255, 255, 255), Default: (0, 0, 0)"
    );
}

// ─── ControlValueSetter Display rendering ───
//
// Like `ControlValueDescription`, `ControlValueSetter::Display` has
// hand-written rendering arms per variant. The setter is what
// callers pass to `CameraDevice::set_control`, and the rendered
// shape lands in `NokhwaError::SetPropertyError` payloads when the
// underlying backend rejects a value. Pin all 10 variant renderings
// so a refactor of one variant doesn't silently change error
// diagnostics that downstream consumers grep on.

#[test]
fn control_value_setter_display_none() {
    assert_eq!(ControlValueSetter::None.to_string(), "Value: None");
}

#[test]
fn control_value_setter_display_integer() {
    assert_eq!(
        ControlValueSetter::Integer(42).to_string(),
        "IntegerValue: 42"
    );
    assert_eq!(
        ControlValueSetter::Integer(-1).to_string(),
        "IntegerValue: -1"
    );
}

#[test]
fn control_value_setter_display_float() {
    assert_eq!(
        ControlValueSetter::Float(1.5).to_string(),
        "FloatValue: 1.5"
    );
}

#[test]
fn control_value_setter_display_boolean() {
    assert_eq!(
        ControlValueSetter::Boolean(true).to_string(),
        "BoolValue: true"
    );
    assert_eq!(
        ControlValueSetter::Boolean(false).to_string(),
        "BoolValue: false"
    );
}

#[test]
fn control_value_setter_display_string() {
    assert_eq!(
        ControlValueSetter::String("hello".to_string()).to_string(),
        "StrValue: hello"
    );
}

#[test]
fn control_value_setter_display_bytes() {
    assert_eq!(
        ControlValueSetter::Bytes(vec![0x01, 0xab, 0xff]).to_string(),
        "BytesValue: [1, ab, ff]"
    );
}

#[test]
fn control_value_setter_display_key_value() {
    assert_eq!(
        ControlValueSetter::KeyValue(1, 2).to_string(),
        "KVValue: (1, 2)"
    );
}

#[test]
fn control_value_setter_display_point() {
    assert_eq!(
        ControlValueSetter::Point(1.5, 2.5).to_string(),
        "PointValue: (1.5, 2.5)"
    );
}

#[test]
fn control_value_setter_display_enum_value() {
    assert_eq!(ControlValueSetter::EnumValue(7).to_string(), "EnumValue: 7");
}

#[test]
fn control_value_setter_display_rgb() {
    assert_eq!(
        ControlValueSetter::RGB(0.5, 1.0, 0.25).to_string(),
        "RGBValue: (0.5, 1, 0.25)"
    );
}

// `ControlValueSetter::RGB` stores three `f64`s and renders them via
// `f64::Display`, which drops trailing `.0` on whole numbers — so
// `RGB(1.0, 2.0, 3.0)` formats as `(1, 2, 3)`, not `(1.0, 2.0, 3.0)`.
// The existing `control_value_setter_display_rgb` only exercises that
// drop on the middle channel, so a regression that introduced
// `{:.1}` width formatting on the first or third channel would
// silently change the output for users with whole-number RGB
// controls. Pin the all-integer case explicitly.
#[test]
fn control_value_setter_display_rgb_all_integer_drops_decimal() {
    assert_eq!(
        ControlValueSetter::RGB(1.0, 2.0, 3.0).to_string(),
        "RGBValue: (1, 2, 3)"
    );
}

// ─── CameraControl Display rendering ───
//
// `CameraControl::Display` composes the renderings of
// `KnownCameraControl::Display` (variant name), the control's name
// field, `ControlValueDescription::Display`, the flag list via
// `Debug`, and the active boolean. This is the canonical log line
// shape backends emit when listing controls; pin it so a regression
// that re-orders the fields breaks the unit test rather than every
// downstream log parser.

#[test]
fn camera_control_display_renders_canonical_log_line() {
    let ctrl = CameraControl::new(
        KnownCameraControl::Brightness,
        "Brightness".to_string(),
        ControlValueDescription::IntegerRange {
            min: 0,
            max: 100,
            value: 50,
            step: 1,
            default: 50,
        },
        vec![KnownCameraControlFlag::Manual],
        true,
    );
    assert_eq!(
        ctrl.to_string(),
        "Control: Brightness, Name: Brightness, Value: (Current: 50, Default: 50, Step: 1, Range: (0, 100)), Flag: [Manual], Active: true"
    );
}

// The canonical-log-line test above only pins `Active: true` and a
// non-empty `Flag: [Manual]` rendering. `Active: false` and an empty
// `Flag: []` are unpinned, yet both surface in real diagnostic output
// (e.g. read-only / disabled controls and controls without flag
// metadata). UI dashboards / log filters that grey-out disabled
// controls match on the literal `"Active: false"` substring; a
// refactor that swapped the boolean rendering to
// `Active: (inactive)` or the empty Vec rendering to `Flag: (none)`
// would pass `camera_control_display_renders_canonical_log_line`
// while silently breaking those filters. Guards
// `nokhwa-core/src/types.rs:1353-1361`.
#[test]
fn camera_control_display_inactive_with_empty_flags() {
    let ctrl = CameraControl::new(
        KnownCameraControl::Contrast,
        "Contrast".to_string(),
        ControlValueDescription::None,
        vec![],
        false,
    );
    assert_eq!(
        ctrl.to_string(),
        "Control: Contrast, Name: Contrast, Value: (None), Flag: [], Active: false"
    );
}

// `buf_*` conversion functions all live behind input-validation guards
// that protect SIMD kernels from out-of-bounds writes. The happy paths
// are exercised by `frame.rs` integration tests, but the guards
// themselves are reachable only when a caller passes a wrong-sized
// slice or odd resolution. Pin every guard so a future "let's relax
// this assertion" tweak surfaces here instead of in a SIMD UB report.

fn assert_process_frame_error(err: NokhwaError, expected_src: FrameFormat) {
    match err {
        NokhwaError::ProcessFrameError { src, .. } => assert_eq!(src, expected_src),
        other => panic!("expected ProcessFrameError, got {other:?}"),
    }
}

#[test]
fn buf_yuyv422_to_rgb_rejects_non_multiple_of_4_input() {
    // YUYV is 4:2:2 — every 2 pixels share a U/V pair packed as
    // [Y0 U Y1 V]. An input length not divisible by 4 cannot
    // describe a complete chunk; the guard rejects it before SIMD
    // walks off the end of the slice.
    let mut dest = vec![0u8; 6];
    let err = buf_yuyv422_to_rgb(&[0u8; 5], &mut dest, false)
        .expect_err("non-multiple-of-4 input must be rejected");
    assert_process_frame_error(err, FrameFormat::YUYV);
}

#[test]
fn buf_yuyv422_to_rgb_rejects_dest_size_mismatch_for_rgb_and_rgba() {
    // 4 input bytes → 2 RGB pixels (6 bytes) or 2 RGBA pixels
    // (8 bytes). A dest sized for the wrong pixel-stride is a
    // common bug when callers swap `rgba` flags without resizing
    // their output buffer.
    let data = [0u8; 4];
    let mut wrong_rgb = vec![0u8; 8];
    let err =
        buf_yuyv422_to_rgb(&data, &mut wrong_rgb, false).expect_err("RGB dest must be 6 bytes");
    assert_process_frame_error(err, FrameFormat::YUYV);

    let mut wrong_rgba = vec![0u8; 6];
    let err =
        buf_yuyv422_to_rgb(&data, &mut wrong_rgba, true).expect_err("RGBA dest must be 8 bytes");
    assert_process_frame_error(err, FrameFormat::YUYV);
}

#[test]
fn buf_nv12_to_rgb_rejects_odd_resolution() {
    // NV12's chroma plane is half-resolution in both dimensions;
    // odd width or height makes the UV plane size non-integral
    // and is rejected up-front.
    let res_odd_w = Resolution::new(3, 2);
    let mut out = vec![0u8; 3 * 3 * 2];
    let err = buf_nv12_to_rgb(res_odd_w, &[0u8; 9], &mut out, false)
        .expect_err("odd width must be rejected");
    assert_process_frame_error(err, FrameFormat::NV12);

    let res_odd_h = Resolution::new(2, 3);
    let err = buf_nv12_to_rgb(res_odd_h, &[0u8; 9], &mut out, false)
        .expect_err("odd height must be rejected");
    assert_process_frame_error(err, FrameFormat::NV12);
}

#[test]
fn buf_nv12_to_rgb_rejects_input_and_output_size_mismatches() {
    // A 4×4 NV12 frame is 4*4 + 4*4/2 = 24 bytes. Pin both the
    // input-size guard (catches truncated USB transfers) and the
    // rgba-dependent output-size guard (catches a caller that
    // sized for RGB but flipped the `rgba` flag to `true`).
    let res = Resolution::new(4, 4);
    let mut out = vec![0u8; 4 * 4 * 3];
    let err = buf_nv12_to_rgb(res, &[0u8; 23], &mut out, false)
        .expect_err("short input must be rejected");
    assert_process_frame_error(err, FrameFormat::NV12);

    // rgba=true requires 4 bytes/pixel, but `out` is sized for 3.
    let err = buf_nv12_to_rgb(res, &[0u8; 24], &mut out, true)
        .expect_err("RGBA output must be 4 bytes/pixel");
    assert_process_frame_error(err, FrameFormat::NV12);
}

#[test]
fn buf_yuyv_extract_luma_rejects_non_multiple_of_4_and_dest_mismatch() {
    // Y-plane extraction must produce one luma byte per pixel
    // (input_len / 2). Both guards (input chunking + dest size)
    // protect the SIMD shuffle from running off the end of the
    // destination slice.
    let mut dest = vec![0u8; 2];
    let err = buf_yuyv_extract_luma(&[0u8; 5], &mut dest)
        .expect_err("non-multiple-of-4 input must be rejected");
    assert_process_frame_error(err, FrameFormat::YUYV);

    let mut wrong_dest = vec![0u8; 3];
    let err =
        buf_yuyv_extract_luma(&[0u8; 4], &mut wrong_dest).expect_err("dest must be input_len / 2");
    assert_process_frame_error(err, FrameFormat::YUYV);
}

#[test]
fn buf_nv12_extract_luma_rejects_input_and_dest_size_mismatches() {
    // NV12 luma extraction is a `copy_from_slice` of the first
    // `w*h` bytes; both guards must fire before the slice index
    // panics. Pin both the input (`w*h*3/2`) and dest (`w*h`)
    // contracts.
    let res = Resolution::new(4, 4);
    let mut dest = vec![0u8; 16];
    let err = buf_nv12_extract_luma(res, &[0u8; 23], &mut dest)
        .expect_err("short NV12 input must be rejected");
    assert_process_frame_error(err, FrameFormat::NV12);

    let mut wrong_dest = vec![0u8; 15];
    let err = buf_nv12_extract_luma(res, &[0u8; 24], &mut wrong_dest)
        .expect_err("dest must be w*h bytes");
    assert_process_frame_error(err, FrameFormat::NV12);
}

// C3: buf_nv12_extract_luma must reject odd dimensions, just like buf_nv12_to_rgb.
// Before the fix, `y_size * 3 / 2` truncated for odd pixel counts (e.g. 3×3 = 9
// pixels → 9 * 3 / 2 = 13 bytes expected instead of the correct 13.5, which is
// not an integer). This made the size-validation imprecise and allowed silent
// extraction on invalid NV12 layouts.
#[test]
fn buf_nv12_extract_luma_rejects_odd_width() {
    // 3×2: width is odd — matches the `buf_nv12_to_rgb` guard style.
    // NV12 with odd dimensions is not a valid layout.
    let res = Resolution::new(3, 2);
    // Provide a plausibly-sized buffer (3*2*3/2 = 9 bytes). The parity
    // check must fire regardless of whether the buffer size looks right.
    let data = vec![0u8; 9];
    let mut dest = vec![0u8; 6];
    let err = buf_nv12_extract_luma(res, &data, &mut dest).expect_err("odd width must be rejected");
    assert_process_frame_error(err, FrameFormat::NV12);
}

#[test]
fn buf_nv12_extract_luma_rejects_odd_height() {
    // 2×3: height is odd.
    let res = Resolution::new(2, 3);
    let data = vec![0u8; 9];
    let mut dest = vec![0u8; 6];
    let err =
        buf_nv12_extract_luma(res, &data, &mut dest).expect_err("odd height must be rejected");
    assert_process_frame_error(err, FrameFormat::NV12);
}

#[test]
fn buf_nv12_extract_luma_even_dimensions_ok() {
    // 4×4 (even width, even height): parity guard passes, and with the
    // right-sized buffers the extraction must succeed and copy the Y plane.
    let res = Resolution::new(4, 4);
    let mut data = Vec::with_capacity(24);
    for y in 1..=16u8 {
        data.push(y);
    }
    for uv in 100..108u8 {
        data.push(uv);
    }
    let mut dest = vec![0u8; 16];
    buf_nv12_extract_luma(res, &data, &mut dest).expect("even 4×4 must succeed");
    assert_eq!(
        dest,
        (1..=16u8).collect::<Vec<_>>(),
        "Y plane must be copied verbatim with no UV bleed"
    );
}

#[test]
fn buf_bgr_to_rgb_rejects_size_mismatches() {
    // BGR-to-RGB is a flat 3-bytes-per-pixel byte shuffle; any pixel
    // count (odd or even dimensions) is valid. The function only
    // rejects mismatches between the declared resolution and the
    // actual buffer lengths. Pin every error branch so a regression
    // never silently indexes past the buffer.
    let res_odd = Resolution::new(5, 4);
    let mut out_odd = vec![0u8; 5 * 4 * 3];
    buf_bgr_to_rgb(res_odd, &[0u8; 5 * 4 * 3], &mut out_odd)
        .expect("odd width must now be accepted");

    let res = Resolution::new(4, 4);
    let mut out_ok = vec![0u8; 4 * 4 * 3];
    let err =
        buf_bgr_to_rgb(res, &[0u8; 47], &mut out_ok).expect_err("short input must be rejected");
    assert_process_frame_error(err, FrameFormat::RAWBGR);

    let mut wrong_out = vec![0u8; 4 * 4 * 4];
    let err = buf_bgr_to_rgb(res, &[0u8; 4 * 4 * 3], &mut wrong_out)
        .expect_err("output must be w*h*3 bytes");
    assert_process_frame_error(err, FrameFormat::RAWBGR);
}

#[test]
fn buf_yuyv_extract_luma_picks_y0_y1_not_chroma() {
    // YUYV is packed `[Y0, U, Y1, V]` per 4-byte chunk; the luma
    // plane must be the bytes at indices 0 and 2 of each chunk.
    // A SIMD shuffle-mask regression that picks U/V instead of Y
    // produces a silently-wrong grayscale image. Pin the byte
    // selection with a known-value pattern where every Y is
    // distinct from every U/V.
    let data: Vec<u8> = vec![
        10, 200, 11, 201, // pixels 0-1: Y=10,11 | U=200, V=201
        12, 202, 13, 203, // pixels 2-3
        14, 204, 15, 205, // pixels 4-5
        16, 206, 17, 207, // pixels 6-7
    ];
    let mut dest = vec![0u8; 8];
    buf_yuyv_extract_luma(&data, &mut dest).expect("extract must succeed");
    assert_eq!(dest, vec![10, 11, 12, 13, 14, 15, 16, 17]);
}

#[test]
fn buf_nv12_extract_luma_copies_y_plane_only() {
    // NV12 is bi-planar: a full-resolution Y plane followed by
    // a half-resolution interleaved UV plane. The extraction is
    // a `copy_from_slice(&data[..w*h])`; an off-by-one in the
    // y_size formula would silently include UV bytes. Use a
    // pattern where Y bytes are 1..=16 and UV bytes are 100+ so
    // any UV leak is unmistakable.
    let res = Resolution::new(4, 4);
    let mut data = Vec::with_capacity(24);
    for y in 1..=16u8 {
        data.push(y);
    }
    for uv in 100..108u8 {
        data.push(uv);
    }
    let mut dest = vec![0u8; 16];
    buf_nv12_extract_luma(res, &data, &mut dest).expect("extract must succeed");
    assert_eq!(
        dest,
        (1..=16u8).collect::<Vec<_>>(),
        "Y plane must be copied verbatim with no UV bleed"
    );
}

#[test]
fn buf_bgr_to_rgb_swaps_b_and_r_channels() {
    // BGR-to-RGB swaps the B and R channels per pixel while
    // leaving G unchanged. A SIMD shuffle-mask inversion would
    // silently produce inverted hues with no error. Use a 4×2
    // frame (8 pixels = 24 bytes) with each pixel's B/G/R
    // distinct so any per-channel mistake is observable.
    let res = Resolution::new(4, 2);
    let mut data = Vec::with_capacity(24);
    for i in 0..8u8 {
        let base = i * 10;
        data.push(base + 1); // B
        data.push(base + 2); // G
        data.push(base + 3); // R
    }
    let mut out = vec![0u8; 24];
    buf_bgr_to_rgb(res, &data, &mut out).expect("convert must succeed");
    for i in 0..8usize {
        let off = i * 3;
        let base = (i as u8) * 10;
        assert_eq!(out[off], base + 3, "pixel {i} R channel");
        assert_eq!(out[off + 1], base + 2, "pixel {i} G channel preserved");
        assert_eq!(out[off + 2], base + 1, "pixel {i} B channel");
    }
}

#[test]
fn buf_bgr_to_rgb_accepts_odd_resolution() {
    // BGR is a flat 3-byte-per-pixel format with no chroma subsampling,
    // so any pixel count — including odd width or height — is valid.
    // This test guards against a regression where an even-dimension
    // check copied from the NV12 path is reintroduced.

    // 3×1: B0G0R0 B1G1R1 B2G2R2 → expect per-pixel B/R swap
    let res = Resolution::new(3, 1);
    let bgr: [u8; 9] = [10, 20, 30, 40, 50, 60, 70, 80, 90];
    let mut rgb = [0u8; 9];
    buf_bgr_to_rgb(res, &bgr, &mut rgb).unwrap();
    assert_eq!(rgb, [30, 20, 10, 60, 50, 40, 90, 80, 70]);

    // 5×3 (both odd): all-ones input must emerge all-ones (G=1 stays,
    // B=1 and R=1 swap but are indistinguishable at a uniform value).
    let res2 = Resolution::new(5, 3);
    let bgr2 = vec![1u8; 5 * 3 * 3];
    let mut rgb2 = vec![0u8; 5 * 3 * 3];
    buf_bgr_to_rgb(res2, &bgr2, &mut rgb2).unwrap();
    assert!(rgb2.iter().all(|&b| b == 1));
}
