//! URL/URI detection + pipeline construction for the GStreamer
//! backend's "open by URL" mode.
//!
//! The device-monitor path covers local cameras; this
//! module covers `rtsp://` / `rtmp://` / `http(s)://` / `file://` and
//! similar URIs that don't show up in `DeviceMonitor`. One pipeline
//! shape covers all of them because GStreamer's [`uridecodebin`]
//! element auto-detects the right source + decoder from the scheme:
//!
//! ```text
//! uridecodebin uri=<URL> ! videoconvert ! appsink
//! ```
//!
//! We deliberately skip `capsfilter` here — unlike local cameras we
//! cannot enumerate format capabilities from a URL ahead of time, so
//! the negotiated format is whatever the first sample delivers and
//! `appsink` is configured with a soft `video/x-raw` caps expectation.
//! `videoconvert` normalises the downstream stream into a format we
//! can report.

use crate::format::video_format_to_frame_format;
use gstreamer::prelude::*;
use gstreamer::{Element, Pipeline, State};
use gstreamer_app::AppSink;
use gstreamer_video::VideoFormat;
use nokhwa_core::{
    buffer::Buffer,
    error::NokhwaError,
    types::{CameraFormat, FrameFormat, Resolution},
};
use std::time::Duration;

/// How long `open()` waits for the first sample to appear before
/// deciding the URL won't yield frames. 10s is generous — RTSP setup
/// over a slow network easily takes 2–3s, and file-based URIs should
/// be instant.
const FIRST_SAMPLE_TIMEOUT: Duration = Duration::from_secs(10);

/// URI-mode pipeline handle. Shape mirrors [`crate::pipeline::PipelineHandle`]
/// but the constructor and source element differ: no `Device`, no
/// `capsfilter`, format is learned from the first sample instead of
/// negotiated ahead of time.
pub(crate) struct UriPipelineHandle {
    pipeline: Pipeline,
    appsink: AppSink,
    source: Element,
    format: CameraFormat,
}

impl UriPipelineHandle {
    pub(crate) fn source(&self) -> &Element {
        &self.source
    }

    pub(crate) fn start(uri: &str) -> Result<Self, NokhwaError> {
        let source = gstreamer::ElementFactory::make("uridecodebin")
            .property("uri", uri)
            .build()
            .map_err(|e| {
                NokhwaError::open_device(uri.to_string(), format!("uridecodebin factory: {e}"))
            })?;

        let convert = gstreamer::ElementFactory::make("videoconvert")
            .build()
            .map_err(|e| {
                NokhwaError::open_device(uri.to_string(), format!("videoconvert factory: {e}"))
            })?;

        // Soft caps — let whatever the decoder produces through after
        // videoconvert, filtered to formats we know how to expose.
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", gstreamer::List::new([&"YUY2", &"NV12", &"GRAY8"]))
            .build();
        let appsink = AppSink::builder()
            .caps(&caps)
            .max_buffers(1)
            .drop(true)
            .build();
        let sink_element: Element = appsink.clone().upcast();
        sink_element.set_property("sync", false);

        let pipeline = Pipeline::new();
        pipeline
            .add(&source)
            .map_err(|e| NokhwaError::OpenStreamError {
                message: format!("Pipeline::add(source): {e}"),
                backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
            })?;
        pipeline
            .add(&convert)
            .map_err(|e| NokhwaError::OpenStreamError {
                message: format!("Pipeline::add(convert): {e}"),
                backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
            })?;
        pipeline
            .add(&sink_element)
            .map_err(|e| NokhwaError::OpenStreamError {
                message: format!("Pipeline::add(appsink): {e}"),
                backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
            })?;

        // uridecodebin has dynamic pads — the src pad appears only
        // after the source is probed and the decoder chain is built.
        // Link the static `convert` <-> `appsink` part immediately,
        // but `source -> convert` has to happen from a pad-added
        // signal callback.
        convert
            .link(&sink_element)
            .map_err(|e| NokhwaError::OpenStreamError {
                message: format!("link convert->appsink: {e}"),
                backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
            })?;
        let convert_weak = convert.downgrade();
        source.connect_pad_added(move |_src, new_pad| {
            let Some(convert) = convert_weak.upgrade() else {
                return;
            };
            let Some(sink_pad) = convert.static_pad("sink") else {
                return;
            };
            if sink_pad.is_linked() {
                return;
            }
            // Only link video pads — uridecodebin also produces audio
            // pads for streams that carry audio, which we don't want.
            //
            // `current_caps()` is the caps already set on the pad; on a
            // freshly-added dynamic pad it can still be `None` until the
            // first caps event flows, even though the pad is a decoded
            // video output. Fall back to `query_caps(None)` (which forces
            // a caps query) so we don't silently decline to link a valid
            // video pad and then time out waiting for a first sample.
            let caps = new_pad
                .current_caps()
                .unwrap_or_else(|| new_pad.query_caps(None));
            let is_video = caps
                .structure(0)
                .is_some_and(|s| s.name().starts_with("video/"));
            if !is_video {
                return;
            }
            let _ = new_pad.link(&sink_pad);
        });

        // Kick the pipeline into Playing. `uridecodebin` negotiates
        // asynchronously; we block on the state change and then the
        // first sample.
        //
        // On any startup failure after `set_state(Playing)` the pipeline
        // may already hold a network socket (RTSP/HTTP) or file handle
        // open. Dropping the local `pipeline` binding does NOT release
        // those — GStreamer only tears them down on the transition back
        // to NULL — and `UriPipelineHandle::Drop` can't run because `Self`
        // isn't constructed yet. Force NULL before returning so the
        // resource isn't leaked. (Mirrors `pipeline.rs`.)
        let state_change = match pipeline.set_state(State::Playing) {
            Ok(sc) => sc,
            Err(e) => {
                let _ = pipeline.set_state(State::Null);
                return Err(NokhwaError::OpenStreamError {
                    message: format!("set_state(Playing): {e}"),
                    backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
                });
            },
        };
        if state_change == gstreamer::StateChangeSuccess::Async {
            let (res, _, _) = pipeline.state(gstreamer::ClockTime::from_seconds(10));
            if let Err(e) = res {
                let _ = pipeline.set_state(State::Null);
                return Err(NokhwaError::OpenStreamError {
                    message: format!("async state wait: {e}"),
                    backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
                });
            }
        }

        // Learn the actual format from the first sample. We can't
        // enumerate `compatible_formats()` from a URL (no probe API
        // short of fully connecting), so the first sample is
        // authoritative.
        let Some(first_sample) = appsink.try_pull_sample(gstreamer::ClockTime::from_nseconds(
            u64::try_from(FIRST_SAMPLE_TIMEOUT.as_nanos()).unwrap_or(u64::MAX),
        )) else {
            let _ = pipeline.set_state(State::Null);
            return Err(NokhwaError::OpenStreamError {
                message: "timed out waiting for the first sample from the URI — \
                          the stream may be unreachable or produce only audio"
                    .to_string(),
                backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
            });
        };
        let format = match sample_format(&first_sample) {
            Ok(f) => f,
            Err(e) => {
                let _ = pipeline.set_state(State::Null);
                return Err(e);
            },
        };

        Ok(Self {
            pipeline,
            appsink,
            source,
            format,
        })
    }

    pub(crate) fn pull_frame(&self) -> Result<Buffer, NokhwaError> {
        let sample = self
            .appsink
            .try_pull_sample(gstreamer::ClockTime::from_seconds(1))
            .ok_or_else(|| NokhwaError::ReadFrameError {
                message: "AppSink::try_pull_sample timed out or hit EOS".to_string(),
                format: Some(self.format.format()),
            })?;
        // Re-derive the format from the current sample's caps so that
        // adaptive streams (HLS/DASH/RTSP that renegotiate resolution
        // mid-stream) report accurate dimensions for each frame. Fall
        // back to `self.format` (the negotiated default from `new()`)
        // only when the sample carries no readable caps.
        let frame_format = sample_format(&sample).unwrap_or(self.format);
        let buffer = sample.buffer().ok_or_else(|| NokhwaError::ReadFrameError {
            message: "Sample carried no GstBuffer".to_string(),
            format: Some(frame_format.format()),
        })?;
        let map = buffer
            .map_readable()
            .map_err(|e| NokhwaError::ReadFrameError {
                message: format!("map_readable: {e}"),
                format: Some(frame_format.format()),
            })?;
        Ok(Buffer::new(
            frame_format.resolution(),
            map.as_slice(),
            frame_format.format(),
        ))
    }

    pub(crate) fn format(&self) -> CameraFormat {
        self.format
    }
}

impl Drop for UriPipelineHandle {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(State::Null);
    }
}

/// Extract resolution + FrameFormat from a sample's caps. URL streams
/// might not advertise a stable framerate, so we default to 30fps and
/// document the limitation.
#[allow(clippy::cast_possible_truncation)]
fn sample_format(sample: &gstreamer::Sample) -> Result<CameraFormat, NokhwaError> {
    let caps = sample
        .caps()
        .ok_or_else(|| NokhwaError::structure("sample caps", "first sample had no caps"))?;
    let structure = caps
        .structure(0)
        .ok_or_else(|| NokhwaError::structure("sample caps structure", "caps has no structure"))?;
    let format_name = structure
        .get::<&str>("format")
        .map_err(|e| NokhwaError::structure("format", e.to_string()))?;
    let frame_format = video_format_to_frame_format(VideoFormat::from_string(format_name))
        .ok_or_else(|| {
            NokhwaError::structure(
                "format",
                format!("videoconvert produced unsupported format: {format_name}"),
            )
        })?;
    let width = structure
        .get::<i32>("width")
        .map_err(|e| NokhwaError::structure("width", e.to_string()))?;
    let height = structure
        .get::<i32>("height")
        .map_err(|e| NokhwaError::structure("height", e.to_string()))?;
    if width <= 0 || height <= 0 {
        return Err(NokhwaError::structure(
            "resolution",
            format!("invalid dimensions {width}x{height}"),
        ));
    }
    // Framerate is optional for URL streams — some RTSP sources
    // advertise 0/1 or skip it entirely. 30fps is a sensible default
    // for the `CameraFormat.frame_rate` field, and frame-timing
    // consumers should rely on the capture timestamp rather than the
    // nominal rate anyway.
    let fps = structure
        .get::<gstreamer::Fraction>("framerate")
        .ok()
        .and_then(|frac| {
            let n = frac.numer();
            let d = frac.denom();
            if n <= 0 || d <= 0 {
                None
            } else if n % d == 0 {
                u32::try_from(n / d).ok()
            } else {
                None
            }
        })
        .unwrap_or(30);

    Ok(CameraFormat::new(
        Resolution::new(width as u32, height as u32),
        frame_format,
        fps,
    ))
}

/// `compatible_fourcc()` for URL-mode sources. `uridecodebin`
/// negotiates exactly one format on the appsink pad — the network
/// peer / file picks the encoding, we don't get to enumerate
/// alternatives — so the compatible-fourcc list is the singleton
/// `[fmt.format()]` (the format we already have). Returning an empty
/// list here would silently break callers that compare it against
/// `compatible_formats()` for the cross-backend invariant
/// (`compatible_fourcc ⊇ compatible_formats.iter().map(|f| f.format())`)
/// pinned in `tests/device_tests.rs`.
#[must_use]
pub(crate) fn compatible_fourcc_from_negotiated(fmt: CameraFormat) -> Vec<FrameFormat> {
    vec![fmt.format()]
}

#[cfg(test)]
mod tests {
    use super::compatible_fourcc_from_negotiated;
    use nokhwa_core::{
        looks_like_url_scheme,
        types::{CameraFormat, FrameFormat, Resolution},
    };

    #[test]
    fn looks_like_uri_detects_all_known_schemes() {
        for s in [
            "rtsp://example.com/stream",
            "rtsps://example.com/stream",
            "rtmp://example.com/live",
            "rtmps://example.com/live",
            "http://example.com/video.mp4",
            "https://example.com/video.mp4",
            "file:///tmp/video.mp4",
            "srt://example.com:8080",
            "udp://239.0.0.1:5004",
            "tcp://example.com:1234",
        ] {
            assert!(looks_like_url_scheme(s), "expected URL: {s}");
        }
    }

    #[test]
    fn looks_like_uri_rejects_non_uri_inputs() {
        for s in [
            "",
            "/dev/video0",
            "video0",
            "Logitech BRIO",
            "ftp://example.com/file",
            "ws://example.com",
            "rtsp",
            "http:/foo",
        ] {
            assert!(!looks_like_url_scheme(s), "expected non-URL: {s}");
        }
    }

    #[test]
    fn looks_like_uri_is_case_insensitive() {
        for s in [
            "RTSP://example.com",
            "Http://example.com",
            "FILE:///tmp/x",
            "RtMpS://example.com",
        ] {
            assert!(looks_like_url_scheme(s), "expected URL (mixed case): {s}");
        }
    }

    #[test]
    fn scheme_list_shape_is_stable() {
        // Each of these MUST be detected. The set is intentionally narrow:
        // we don't want to treat a display name with `:` as a URL.
        let mirror = [
            "rtsp://", "rtsps://", "rtmp://", "rtmps://", "http://", "https://", "file://",
            "srt://", "udp://", "tcp://",
        ];
        for prefix in mirror {
            assert!(
                looks_like_url_scheme(prefix),
                "{prefix} should be a URL prefix"
            );
        }
        // And these intentionally are NOT in the list.
        for unsupported in ["ftp://", "ws://", "wss://", "data:", "mms://"] {
            assert!(
                !looks_like_url_scheme(unsupported),
                "{unsupported} unexpectedly recognised"
            );
        }
    }

    #[test]
    fn compatible_fourcc_from_negotiated_returns_singleton() {
        for f in [
            FrameFormat::MJPEG,
            FrameFormat::YUYV,
            FrameFormat::NV12,
            FrameFormat::GRAY,
            FrameFormat::RAWRGB,
            FrameFormat::RAWBGR,
        ] {
            let cf = CameraFormat::new(Resolution::new(640, 480), f, 30);
            let v = compatible_fourcc_from_negotiated(cf);
            assert_eq!(v, vec![f], "wrong singleton for {f:?}");
        }
    }

    /// `sample_format` (`uri.rs:240`) has 6 distinct error branches plus
    /// a happy path with framerate fallback logic, and zero direct
    /// coverage. The function is the URL-mode counterpart of
    /// `format::caps_to_camera_formats` — `uridecodebin` negotiates
    /// exactly one format and we extract its description from the
    /// `appsink`'s first sample. A regression in any branch would
    /// silently produce wrong-shaped `CameraFormat`s for every URL
    /// source.
    ///
    /// The tests below construct `gstreamer::Sample` values with
    /// hand-crafted Caps to drive each branch deterministically.
    /// They use the same `Once`-init pattern as `format::tests`.
    use std::sync::Once;

    fn ensure_gst_init() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            gstreamer::init().expect("gstreamer::init() must succeed in tests");
        });
    }

    fn sample_with_caps(caps: &gstreamer::Caps) -> gstreamer::Sample {
        gstreamer::Sample::builder().caps(caps).build()
    }

    /// Branch 1 — `sample.caps()` returns `None`. Pin the resulting
    /// `StructureError`'s exact `structure` and `error` fields so a
    /// regression that, e.g., changed the variant to `GeneralError`
    /// or rephrased `"first sample had no caps"` (a contract that
    /// surface debugging tools and downstream tests may quote) gets
    /// caught.
    #[test]
    fn sample_format_errors_when_caps_missing() {
        ensure_gst_init();
        let sample = gstreamer::Sample::builder().build();
        let err = super::sample_format(&sample).unwrap_err();
        match err {
            nokhwa_core::error::NokhwaError::StructureError { structure, error } => {
                assert_eq!(structure, "sample caps");
                assert_eq!(error, "first sample had no caps");
            },
            other => panic!("expected StructureError, got {other:?}"),
        }
    }

    /// Branch 2 — `caps.structure(0)` returns `None` (caps with zero
    /// structures). Pin the second-tier `StructureError` field
    /// distinct from the no-caps branch — both error messages flow
    /// into the same user-visible `Display`, but the `structure`
    /// field is the diagnostic key.
    #[test]
    fn sample_format_errors_when_caps_have_no_structure() {
        ensure_gst_init();
        let caps = gstreamer::Caps::new_empty();
        let sample = sample_with_caps(&caps);
        let err = super::sample_format(&sample).unwrap_err();
        match err {
            nokhwa_core::error::NokhwaError::StructureError { structure, error } => {
                assert_eq!(structure, "sample caps structure");
                assert_eq!(error, "caps has no structure");
            },
            other => panic!("expected StructureError, got {other:?}"),
        }
    }

    /// Branch 3 — `format` field missing from the structure. The
    /// `gstreamer::Structure::get` error string format is GStreamer's
    /// own (we forward `e.to_string()` verbatim), so we only pin the
    /// `structure` field — a regression that silently swallowed the
    /// inner error or relabeled the diagnostic key would slip past
    /// any check tied solely to the wrapper variant.
    #[test]
    fn sample_format_errors_when_format_field_missing() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("width", 640i32)
            .field("height", 480i32)
            .build();
        let sample = sample_with_caps(&caps);
        let err = super::sample_format(&sample).unwrap_err();
        match err {
            nokhwa_core::error::NokhwaError::StructureError { structure, .. } => {
                assert_eq!(structure, "format");
            },
            other => panic!("expected StructureError, got {other:?}"),
        }
    }

    /// Branch 4 — the `format` field is a string that
    /// `VideoFormat::from_string` can't decode (or that
    /// `video_format_to_frame_format` returns `None` for, e.g.
    /// 16-bit grayscale). Pin the canonical `"videoconvert produced
    /// unsupported format: {format_name}"` interpolation — the
    /// `format_name` echoes the user-visible diagnostic and a
    /// regression that dropped it would leave operators staring at
    /// "unsupported format" with no clue which name they got.
    #[test]
    fn sample_format_errors_on_unsupported_video_format() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "GRAY16_BE")
            .field("width", 640i32)
            .field("height", 480i32)
            .build();
        let sample = sample_with_caps(&caps);
        let err = super::sample_format(&sample).unwrap_err();
        match err {
            nokhwa_core::error::NokhwaError::StructureError { structure, error } => {
                assert_eq!(structure, "format");
                assert_eq!(error, "videoconvert produced unsupported format: GRAY16_BE");
            },
            other => panic!("expected StructureError, got {other:?}"),
        }
    }

    /// Branch 5 — `width <= 0 || height <= 0`. The structured check
    /// fires after both fields parse cleanly; pin the canonical
    /// `"invalid dimensions {w}x{h}"` interpolation so a regression
    /// that, e.g., changed the separator from `x` to `*` or dropped
    /// the dimensions from the message gets caught.
    #[test]
    fn sample_format_errors_on_non_positive_dimensions() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "YUY2")
            .field("width", 0i32)
            .field("height", 480i32)
            .build();
        let sample = sample_with_caps(&caps);
        let err = super::sample_format(&sample).unwrap_err();
        match err {
            nokhwa_core::error::NokhwaError::StructureError { structure, error } => {
                assert_eq!(structure, "resolution");
                assert_eq!(error, "invalid dimensions 0x480");
            },
            other => panic!("expected StructureError, got {other:?}"),
        }
    }

    /// Happy-path branch with no `framerate` field — RTSP sources
    /// often don't advertise one. Documented behavior: fall back to
    /// 30 FPS. A regression that, e.g., switched the fallback to 0
    /// or to `u32::MAX` would silently break downstream code that
    /// keys on `frame_rate()` (e.g. `RunnerConfig` poll-interval
    /// tuning, `CameraFormat::Display`).
    #[test]
    fn sample_format_uses_thirty_fps_fallback_when_framerate_missing() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "YUY2")
            .field("width", 640i32)
            .field("height", 480i32)
            .build();
        let sample = sample_with_caps(&caps);
        let cf = super::sample_format(&sample).expect("happy path");
        assert_eq!(cf.frame_rate(), 30);
        assert_eq!(cf.width(), 640);
        assert_eq!(cf.height(), 480);
        assert_eq!(cf.format(), FrameFormat::YUYV);
    }

    /// Happy-path branch where the framerate fraction has a non-
    /// integer value (`30000/1001` ≈ 29.97 NTSC). Documented
    /// behavior: fall back to 30 FPS rather than truncate. Pin so a
    /// refactor that turned the `n % d == 0` check into an
    /// integer-division truncation (rounding down to 29 FPS, which
    /// is *not* on the common-FPS table and would mis-route every
    /// downstream rate-aware code path) gets caught.
    #[test]
    fn sample_format_falls_back_when_framerate_is_non_integer() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "YUY2")
            .field("width", 640i32)
            .field("height", 480i32)
            .field("framerate", gstreamer::Fraction::new(30_000, 1_001))
            .build();
        let sample = sample_with_caps(&caps);
        let cf = super::sample_format(&sample).expect("happy path");
        assert_eq!(cf.frame_rate(), 30);
    }

    /// Happy-path branch where the framerate is zero or negative
    /// (`n <= 0` or `d <= 0`). Documented behavior: fall back to 30
    /// FPS. A `0/1` fraction is a real-world value reported by
    /// stalled RTSP streams.
    #[test]
    fn sample_format_falls_back_on_zero_or_negative_framerate() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "YUY2")
            .field("width", 640i32)
            .field("height", 480i32)
            .field("framerate", gstreamer::Fraction::new(0, 1))
            .build();
        let sample = sample_with_caps(&caps);
        let cf = super::sample_format(&sample).expect("happy path");
        assert_eq!(cf.frame_rate(), 30);
    }

    /// Happy-path branch where the framerate is a clean integer
    /// (`60/1`). Documented behavior: pass through verbatim — pin
    /// so the fallback isn't triggered too eagerly.
    #[test]
    fn sample_format_preserves_clean_integer_framerate() {
        ensure_gst_init();
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "NV12")
            .field("width", 1920i32)
            .field("height", 1080i32)
            .field("framerate", gstreamer::Fraction::new(60, 1))
            .build();
        let sample = sample_with_caps(&caps);
        let cf = super::sample_format(&sample).expect("happy path");
        assert_eq!(cf.frame_rate(), 60);
        assert_eq!(cf.width(), 1920);
        assert_eq!(cf.height(), 1080);
        assert_eq!(cf.format(), FrameFormat::NV12);
    }
}
