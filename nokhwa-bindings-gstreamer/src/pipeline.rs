//! GStreamer pipeline lifecycle for local-device streaming.
//!
//! Pipeline shape:
//!
//! ```text
//! source (device.create_element) ! capsfilter ! videoconvert ! appsink
//! ```
//!
//! `capsfilter` locks the negotiated `video/x-raw` format/resolution/
//! framerate so downstream negotiation doesn't drift. `videoconvert`
//! is a safety net for sources that won't hand us exactly the format
//! we asked for (rare, but the cost is cheap). `appsink` is the
//! egress — we pull samples synchronously via
//! [`AppSink::pull_sample`].
//!
//! AppSink is configured for low-latency "grab the latest frame"
//! semantics: `max-buffers=1`, `drop=true`, `sync=false`. That matches
//! the other backends (MSMF / V4L / AVF) — nokhwa's `FrameSource`
//! contract is "give me the freshest frame," not "give me every frame
//! in order."

use crate::format::frame_format_to_video_format;
use gstreamer::prelude::*;
use gstreamer::{Caps, Device, Element, Fraction, Pipeline, State};
use gstreamer_app::AppSink;
use gstreamer_video::VideoFormat;
use nokhwa_core::{
    buffer::Buffer,
    error::NokhwaError,
    types::{CameraFormat, FrameFormat},
};
use std::time::Duration;

/// How long [`PipelineHandle::pull_frame`] blocks waiting for the next
/// sample before returning [`NokhwaError::ReadFrameError`]. Matches
/// V4L's `read_timeout` (1 second) — long enough for a slow source to
/// warm up, short enough to avoid wedging a misbehaving pipeline.
const PULL_TIMEOUT: Duration = Duration::from_secs(1);

/// Owning handle to a live GStreamer pipeline.
///
/// Drops `set_state(Null)` automatically so a forgotten `close()` call
/// doesn't leak a playing pipeline across subsequent device opens.
pub(crate) struct PipelineHandle {
    pipeline: Pipeline,
    appsink: AppSink,
    source: Element,
    format: CameraFormat,
}

impl PipelineHandle {
    /// Access the source element for control introspection + writes.
    /// On Linux this is `v4l2src`; on Windows `ksvideosrc` /
    /// `mfvideosrc`; on macOS `avfvideosrc`.
    pub(crate) fn source(&self) -> &Element {
        &self.source
    }
}

impl PipelineHandle {
    /// Build + start a pipeline for `device` negotiated to `format`.
    ///
    /// Synchronously waits for the `Playing` state change so that the
    /// very first `pull_frame` call sees a live buffer queue rather
    /// than racing a half-initialised pipeline.
    pub(crate) fn start(
        device: &Device,
        format: CameraFormat,
        extra_controls: Option<gstreamer::Structure>,
    ) -> Result<Self, NokhwaError> {
        let video_format = frame_format_to_video_format(format.format()).ok_or_else(|| {
            NokhwaError::set_property(
                "FrameFormat",
                format!("{:?}", format.format()),
                "not supported by the GStreamer pipeline",
            )
        })?;

        let caps_value = caps_for(format, video_format);

        let source = device.create_element(None).map_err(|e| {
            NokhwaError::open_device(
                device.display_name().to_string(),
                format!("Device::create_element failed: {e}"),
            )
        })?;

        // Apply extra-controls before state leaves NULL — v4l2src reads
        // this property during the transition to READY and dispatches
        // the corresponding V4L2 VIDIOC_S_CTRL ioctls. Best-effort;
        // non-v4l2 source elements simply ignore the property.
        if let Some(structure) = extra_controls {
            // `find_property` keeps this safe on source elements that
            // don't know what `extra-controls` is (everything other
            // than v4l2src): skip silently instead of asserting.
            if source.find_property("extra-controls").is_some() {
                source.set_property("extra-controls", &structure);
            }
        }

        let capsfilter = gstreamer::ElementFactory::make("capsfilter")
            .property("caps", caps_value.clone())
            .build()
            .map_err(|e| element_err("capsfilter", &e.to_string()))?;

        let convert = gstreamer::ElementFactory::make("videoconvert")
            .build()
            .map_err(|e| element_err("videoconvert", &e.to_string()))?;

        let appsink = AppSink::builder()
            .caps(&caps_value)
            .max_buffers(1)
            .drop(true)
            .build();
        // `sync` is a property on `BaseSink`, the parent of `AppSink`.
        // Setting it to `false` means the sink hands frames up
        // immediately on arrival instead of waiting for the pipeline
        // clock — correct semantics for "grab latest frame."
        let sink_element: Element = appsink.clone().upcast();
        sink_element.set_property("sync", false);

        let pipeline = Pipeline::new();
        pipeline
            .add(&source)
            .map_err(|err| element_err("Pipeline::add(source)", &err.to_string()))?;
        pipeline
            .add(&capsfilter)
            .map_err(|err| element_err("Pipeline::add(capsfilter)", &err.to_string()))?;
        pipeline
            .add(&convert)
            .map_err(|err| element_err("Pipeline::add(convert)", &err.to_string()))?;
        pipeline
            .add(&sink_element)
            .map_err(|err| element_err("Pipeline::add(appsink)", &err.to_string()))?;
        source
            .link(&capsfilter)
            .map_err(|err| element_err("link source->capsfilter", &err.to_string()))?;
        capsfilter
            .link(&convert)
            .map_err(|err| element_err("link capsfilter->convert", &err.to_string()))?;
        convert
            .link(&sink_element)
            .map_err(|err| element_err("link convert->appsink", &err.to_string()))?;

        // On any startup failure the pipeline may already be in
        // PAUSED/PLAYING with the source element (the camera device
        // handle) reffed. Dropping the local `pipeline` binding does
        // NOT release those elements — GStreamer only tears them down on
        // the transition back to NULL — and `PipelineHandle::Drop` can't
        // run because `Self` isn't constructed yet. Force NULL before
        // returning so the device handle isn't leaked.
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
            let (res, _, _) = pipeline.state(gstreamer::ClockTime::from_seconds(5));
            if let Err(e) = res {
                let _ = pipeline.set_state(State::Null);
                return Err(NokhwaError::OpenStreamError {
                    message: format!("async state wait: {e}"),
                    backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
                });
            }
        }

        Ok(Self {
            pipeline,
            appsink,
            source,
            format,
        })
    }

    /// Pull the next ready sample and copy it into a nokhwa
    /// [`Buffer`]. Blocks up to [`PULL_TIMEOUT`]; translates timeout
    /// and EOS into [`NokhwaError::ReadFrameError`].
    pub(crate) fn pull_frame(&self) -> Result<Buffer, NokhwaError> {
        let sample = self
            .appsink
            .try_pull_sample(gstreamer::ClockTime::from_nseconds(
                u64::try_from(PULL_TIMEOUT.as_nanos()).unwrap_or(u64::MAX),
            ))
            .ok_or_else(|| NokhwaError::ReadFrameError {
                message: "AppSink::try_pull_sample timed out or hit EOS".to_string(),
                format: Some(self.format.format()),
            })?;

        let buffer = sample.buffer().ok_or_else(|| NokhwaError::ReadFrameError {
            message: "Sample carried no GstBuffer".to_string(),
            format: Some(self.format.format()),
        })?;

        let map = buffer
            .map_readable()
            .map_err(|e| NokhwaError::ReadFrameError {
                message: format!("map_readable: {e}"),
                format: Some(self.format.format()),
            })?;

        Ok(Buffer::new(
            self.format.resolution(),
            map.as_slice(),
            self.format.format(),
        ))
    }
}

impl Drop for PipelineHandle {
    fn drop(&mut self) {
        // Best-effort — if Null transition fails we've already lost
        // control of the pipeline, and bubbling the error up past Drop
        // isn't possible anyway.
        let _ = self.pipeline.set_state(State::Null);
    }
}

/// `Caps` for the negotiated format. Both the source-side capsfilter
/// and the appsink use the same caps so `videoconvert` sees matching
/// sink/src pads and is a no-op on the happy path.
fn caps_for(fmt: CameraFormat, video_format: VideoFormat) -> Caps {
    #[allow(clippy::cast_possible_wrap)]
    Caps::builder("video/x-raw")
        .field("format", video_format.to_str().as_str())
        .field("width", fmt.width() as i32)
        .field("height", fmt.height() as i32)
        .field("framerate", Fraction::new(fmt.frame_rate() as i32, 1))
        .build()
}

fn element_err(what: &str, why: &str) -> NokhwaError {
    NokhwaError::OpenStreamError {
        message: format!("{what}: {why}"),
        backend: Some(nokhwa_core::types::ApiBackend::GStreamer),
    }
}

/// Ensure GStreamer is initialized. Idempotent — safe to call from
/// multiple call sites. Standardizes the `NokhwaError` wording so
/// failures are diagnosable regardless of entry point.
pub(crate) fn ensure_gst_init() -> Result<(), NokhwaError> {
    gstreamer::init().map_err(|e| NokhwaError::general(format!("gstreamer init failed: {e}")))
}

/// Open a `DeviceMonitor` filtered to `Video/Source` + `video/x-raw`,
/// snapshot the current device list, and stop the monitor. Used by
/// both `query()` and `find_device()` to keep enumeration logic and
/// error wording in one place.
pub(crate) fn snapshot_video_devices() -> Result<Vec<gstreamer::Device>, NokhwaError> {
    use gstreamer::DeviceMonitor;

    ensure_gst_init()?;
    let monitor = DeviceMonitor::new();
    let caps = Caps::builder("video/x-raw").build();
    // Returning None from add_filter means the filter slot could not
    // be installed. A zero-filter monitor would surface every device
    // on the host, including audio sources — treat it as a fatal
    // enumeration error rather than silently widening the query.
    if monitor
        .add_filter(Some("Video/Source"), Some(&caps))
        .is_none()
    {
        return Err(NokhwaError::structure(
            "DeviceMonitor filter Video/Source",
            "add_filter returned None",
        ));
    }
    monitor
        .start()
        .map_err(|e| NokhwaError::general(format!("DeviceMonitor::start failed: {e}")))?;
    let devices = monitor.devices().into_iter().collect();
    // Stop the monitor before returning — leaked monitors hold
    // references to GStreamer plugins that subsequent calls expect
    // to be free.
    monitor.stop();
    Ok(devices)
}

/// Walk the live monitor a second time to find the device the caller
/// enumerated via [`crate::query`]. We pick by `display_name` because
/// the original `query` only stored that in `CameraInfo.human_name`;
/// falling back to positional index lets the common "first camera"
/// path work even when two devices share a display name.
pub(crate) fn find_device(
    display_name: &str,
    positional_index: u32,
) -> Result<Device, NokhwaError> {
    let devices = snapshot_video_devices()?;

    if !display_name.is_empty() {
        if let Some(d) = devices
            .iter()
            .find(|d| d.display_name().as_str() == display_name)
        {
            return Ok(d.clone());
        }
    }
    devices
        .into_iter()
        .nth(positional_index as usize)
        .ok_or_else(|| {
            NokhwaError::open_device(
                format!("index={positional_index} name={display_name}"),
                "device not found",
            )
        })
}

/// Pull the full capability set of a device as a flat
/// `Vec<CameraFormat>`.
pub(crate) fn compatible_formats(device: &Device) -> Vec<CameraFormat> {
    match device.caps() {
        Some(caps) => crate::format::caps_to_camera_formats(&caps),
        None => Vec::new(),
    }
}

/// Pick the best-matching format for `req` from `candidates`. Panics
/// with an error if nothing matches — same contract as MSMF's
/// `set_format`.
pub(crate) fn resolve_format(
    candidates: &[CameraFormat],
    req: &nokhwa_core::types::RequestedFormat,
) -> Result<CameraFormat, NokhwaError> {
    if candidates.is_empty() {
        return Err(NokhwaError::open_device(
            "GStreamer device",
            "no compatible formats",
        ));
    }
    req.fulfill(candidates).ok_or_else(|| {
        NokhwaError::open_device(
            "GStreamer device",
            format!("no format in the device's caps satisfied the request: {candidates:?}"),
        )
    })
}

/// Distinct `FrameFormat`s across a candidate list, sorted in
/// `FrameFormat`'s `Ord` order. Mirrors the V4L / AVFoundation / MSMF
/// shape (`collect → sort → dedup`) so callers see a stable
/// cross-backend ordering regardless of how the underlying API
/// enumerated its caps.
pub(crate) fn compatible_fourcc(candidates: &[CameraFormat]) -> Vec<FrameFormat> {
    let mut out: Vec<FrameFormat> = candidates.iter().map(CameraFormat::format).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nokhwa_core::types::{RequestedFormat, RequestedFormatType, Resolution};

    fn fmt(w: u32, h: u32, ff: FrameFormat, fps: u32) -> CameraFormat {
        CameraFormat::new(Resolution::new(w, h), ff, fps)
    }

    #[test]
    fn compatible_fourcc_empty_returns_empty() {
        assert_eq!(compatible_fourcc(&[]), Vec::<FrameFormat>::new());
    }

    #[test]
    fn compatible_fourcc_dedupes_and_sorts() {
        // Three duplicates of YUYV mixed with one NV12 and one MJPEG —
        // result must be one of each, sorted in `FrameFormat::Ord` order.
        let candidates = [
            fmt(640, 480, FrameFormat::YUYV, 30),
            fmt(1280, 720, FrameFormat::NV12, 30),
            fmt(640, 480, FrameFormat::YUYV, 60),
            fmt(1920, 1080, FrameFormat::MJPEG, 30),
            fmt(1280, 720, FrameFormat::YUYV, 30),
        ];
        let out = compatible_fourcc(&candidates);
        let mut expected = vec![FrameFormat::YUYV, FrameFormat::NV12, FrameFormat::MJPEG];
        expected.sort();
        assert_eq!(out, expected);
    }

    #[test]
    fn compatible_fourcc_singleton_returns_singleton() {
        let candidates = [fmt(640, 480, FrameFormat::GRAY, 30)];
        assert_eq!(compatible_fourcc(&candidates), vec![FrameFormat::GRAY]);
    }

    #[test]
    fn compatible_fourcc_ordering_matches_frame_format_ord() {
        // Construct candidates in reverse `FrameFormat::Ord` order — the
        // sort must place them in the canonical cross-backend ordering.
        let candidates = [
            fmt(640, 480, FrameFormat::RAWBGR, 30),
            fmt(640, 480, FrameFormat::RAWRGB, 30),
            fmt(640, 480, FrameFormat::GRAY, 30),
            fmt(640, 480, FrameFormat::NV12, 30),
            fmt(640, 480, FrameFormat::MJPEG, 30),
            fmt(640, 480, FrameFormat::YUYV, 30),
        ];
        let mut got = compatible_fourcc(&candidates);
        let mut expected = got.clone();
        expected.sort();
        assert_eq!(got, expected);
        // Idempotent: running it again is identical.
        got = compatible_fourcc(&got.iter().map(|f| fmt(1, 1, *f, 30)).collect::<Vec<_>>());
        assert_eq!(got, expected);
    }

    #[test]
    fn resolve_format_empty_candidates_errors_with_open_device() {
        // Pin the error string verbatim. Previously this used
        // `.contains("no compatible formats")` which would still pass
        // if a future refactor expanded the message to include extra
        // context (e.g. `"no compatible formats; got 0 candidates"`)
        // — that drift would silently invalidate downstream tests
        // that quote the canonical wording, and would diverge the
        // GStreamer message from the V4L / MSMF / AVFoundation paths
        // that callers may switch on by string content.
        let req =
            RequestedFormat::with_formats(RequestedFormatType::AbsoluteHighestResolution, &[]);
        let err = resolve_format(&[], &req).unwrap_err();
        match err {
            NokhwaError::OpenDeviceError { device, error } => {
                assert_eq!(device, "GStreamer device");
                assert_eq!(error, "no compatible formats");
            },
            other => panic!("expected OpenDeviceError, got {other:?}"),
        }
    }

    #[test]
    fn resolve_format_no_matching_format_errors() {
        // Candidates only carry MJPEG, but the request only accepts YUYV
        // — `fulfill` returns None and `resolve_format` must surface an
        // OpenDeviceError that includes the candidate list verbatim.
        //
        // The previous version checked
        // `error.contains("no format in the device's caps satisfied the
        // request")` and `error.contains("MJPEG")`. Both could pass
        // even after meaningful regressions:
        //   - rewording "satisfied the request" → "matched the request"
        //     (or any subtler tweak) would only break consumers who
        //     quote the canonical phrase, and `contains("MJPEG")`
        //     still trivially holds because every candidate prints its
        //     `FrameFormat` Debug;
        //   - swapping the `{candidates:?}` interpolation for a
        //     summary like `"{} candidates"`.len()` (a refactor that
        //     would superficially "shorten the message") would still
        //     drop the user's diagnostic — but `contains("MJPEG")`
        //     would fire only if the count format was lucky.
        // Pin the exact prefix `"no format in the device's caps
        // satisfied the request: "` and verify the suffix is the
        // `Debug` form of the `candidates` slice — that's the
        // documented contract: `format!("…: {candidates:?}")`.
        let candidates = [
            fmt(640, 480, FrameFormat::MJPEG, 30),
            fmt(1280, 720, FrameFormat::MJPEG, 30),
        ];
        let req = RequestedFormat::with_formats(
            RequestedFormatType::AbsoluteHighestResolution,
            &[FrameFormat::YUYV],
        );
        let err = resolve_format(&candidates, &req).unwrap_err();
        match err {
            NokhwaError::OpenDeviceError { device, error } => {
                assert_eq!(device, "GStreamer device");
                let expected =
                    format!("no format in the device's caps satisfied the request: {candidates:?}");
                assert_eq!(error, expected);
            },
            other => panic!("expected OpenDeviceError, got {other:?}"),
        }
    }

    #[test]
    fn resolve_format_picks_highest_resolution() {
        let candidates = [
            fmt(640, 480, FrameFormat::YUYV, 30),
            fmt(1920, 1080, FrameFormat::YUYV, 30),
            fmt(1280, 720, FrameFormat::YUYV, 30),
        ];
        let req = RequestedFormat::with_formats(
            RequestedFormatType::AbsoluteHighestResolution,
            &[FrameFormat::YUYV],
        );
        let chosen = resolve_format(&candidates, &req).unwrap();
        assert_eq!(chosen.resolution(), Resolution::new(1920, 1080));
        assert_eq!(chosen.format(), FrameFormat::YUYV);
    }

    #[test]
    fn resolve_format_picks_highest_framerate_at_max_resolution() {
        // Two entries share the max resolution; `fulfill` must tie-break
        // by framerate (highest wins).
        let candidates = [
            fmt(1920, 1080, FrameFormat::YUYV, 30),
            fmt(1920, 1080, FrameFormat::YUYV, 60),
            fmt(1280, 720, FrameFormat::YUYV, 120),
        ];
        let req = RequestedFormat::with_formats(
            RequestedFormatType::AbsoluteHighestResolution,
            &[FrameFormat::YUYV],
        );
        let chosen = resolve_format(&candidates, &req).unwrap();
        assert_eq!(chosen.resolution(), Resolution::new(1920, 1080));
        assert_eq!(chosen.frame_rate(), 60);
    }

    #[test]
    fn resolve_format_exact_returns_exact_match() {
        let target = fmt(1280, 720, FrameFormat::NV12, 30);
        let candidates = [
            fmt(640, 480, FrameFormat::YUYV, 30),
            target,
            fmt(1920, 1080, FrameFormat::MJPEG, 60),
        ];
        let req = RequestedFormat::with_formats(
            RequestedFormatType::Exact(target),
            &[FrameFormat::NV12, FrameFormat::YUYV, FrameFormat::MJPEG],
        );
        let chosen = resolve_format(&candidates, &req).unwrap();
        assert_eq!(chosen, target);
    }

    #[test]
    fn resolve_format_filters_by_wanted_decoder_list() {
        // Both formats are at 1920x1080 and the request asks for the
        // absolute-highest resolution, but the wanted-decoder list only
        // permits MJPEG — we must not pick the (otherwise tied) YUYV
        // entry.
        let candidates = [
            fmt(1920, 1080, FrameFormat::YUYV, 60),
            fmt(1920, 1080, FrameFormat::MJPEG, 30),
        ];
        let req = RequestedFormat::with_formats(
            RequestedFormatType::AbsoluteHighestResolution,
            &[FrameFormat::MJPEG],
        );
        let chosen = resolve_format(&candidates, &req).unwrap();
        assert_eq!(chosen.format(), FrameFormat::MJPEG);
    }

    /// `element_err` (pipeline.rs:224) wraps a "what failed: why"
    /// description into `OpenStreamError` with the GStreamer backend
    /// tag set. Used by 9 distinct call sites in `PipelineHandle::start`
    /// (capsfilter / videoconvert / source / Pipeline::add(_) /
    /// Element::link(_)). The variant + backend tag + canonical
    /// `"{what}: {why}"` interpolation are the contract that
    /// downstream code may switch on (e.g. tests that pattern-match
    /// `OpenStreamError { backend: Some(GStreamer), .. }` to attribute
    /// failures to the right backend).
    ///
    /// A regression that, e.g., dropped the backend tag, swapped the
    /// variant for `GeneralError`, or rewrote the interpolation
    /// (`format!("{what}: {why}")` → `format!("[{what}] {why}")`)
    /// would silently break the contract. Pin all three.
    #[test]
    fn element_err_uses_open_stream_with_gstreamer_backend() {
        use nokhwa_core::types::ApiBackend;

        let err = element_err("capsfilter", "no plugin");
        match err {
            NokhwaError::OpenStreamError { message, backend } => {
                assert_eq!(message, "capsfilter: no plugin");
                assert_eq!(backend, Some(ApiBackend::GStreamer));
            },
            other => panic!("expected OpenStreamError, got {other:?}"),
        }

        // The 9 production call sites pass varied `what` strings —
        // the helper must preserve them verbatim regardless of
        // formatting (no trimming, lowercasing, prefixing).
        let err = element_err("link convert->appsink", "negotiation failed");
        match err {
            NokhwaError::OpenStreamError { message, backend } => {
                assert_eq!(message, "link convert->appsink: negotiation failed");
                assert_eq!(backend, Some(ApiBackend::GStreamer));
            },
            other => panic!("expected OpenStreamError, got {other:?}"),
        }
    }

    /// The `Display` implementation on `NokhwaError::OpenStreamError`
    /// folds the optional `backend` tag into the user-facing message
    /// when `Some(_)`. Pin the full Display string so a regression
    /// that, e.g., dropped the parenthetical backend hint, swapped the
    /// preposition (`"by backend"` → `"of backend"`), or renamed the
    /// variant's `#[error(...)]` template fires here. Mirror of
    /// `unsupported_returns_unsupported_operation_error` in
    /// `controls.rs`.
    #[test]
    fn element_err_display_form_pins_backend_parenthetical() {
        let err = element_err("capsfilter", "no plugin");
        assert_eq!(
            format!("{err}"),
            "Could not open device stream (backend GStreamer): capsfilter: no plugin"
        );
    }

    /// `caps_for` (pipeline.rs:214) builds the `video/x-raw` Caps that
    /// `capsfilter` uses to lock the negotiation. The structure name
    /// (`video/x-raw`) and the four fields (`format` / `width` /
    /// `height` / `framerate`) are the documented contract:
    /// `capsfilter` enforces them by failing pipeline negotiation if
    /// the upstream source can't produce a matching cap. A regression
    /// that, e.g., dropped the framerate field would leave negotiation
    /// underconstrained and the resulting pipeline would run at
    /// whatever framerate the source picked — silently violating the
    /// `RequestedFormat` the user asked for.
    ///
    /// The `Caps::to_string()` form is GStreamer's documented
    /// serialization (`gst-launch` parses it directly), so pin the
    /// exact rendered string for a representative `(YUYV, 1920x1080,
    /// 30 FPS)` input.
    #[test]
    fn caps_for_emits_video_x_raw_with_all_four_fields() {
        ensure_gst_init();
        let fmt_in = fmt(1920, 1080, FrameFormat::YUYV, 30);
        let caps = caps_for(fmt_in, VideoFormat::Yuy2);

        assert_eq!(caps.size(), 1, "caps_for must emit exactly one Structure");
        let structure = caps.structure(0).expect("Some after size>0");
        assert_eq!(structure.name(), "video/x-raw");
        assert_eq!(structure.get::<&str>("format").unwrap(), "YUY2");
        assert_eq!(structure.get::<i32>("width").unwrap(), 1920);
        assert_eq!(structure.get::<i32>("height").unwrap(), 1080);
        assert_eq!(
            structure.get::<Fraction>("framerate").unwrap(),
            Fraction::new(30, 1)
        );
    }

    /// `caps_for` is a pure function of its two inputs. Pin a second
    /// shape (`MJPEG, 640x480, 60 FPS`) to catch the alternative
    /// regression where the helper hard-codes some dimension instead
    /// of forwarding the input. (E.g. a refactor that swapped
    /// `fmt.width()` for `1920` to "fix" a different bug would still
    /// pass `caps_for_emits_video_x_raw_with_all_four_fields`.)
    #[test]
    fn caps_for_forwards_each_input_field_independently() {
        ensure_gst_init();
        let fmt_in = fmt(640, 480, FrameFormat::MJPEG, 60);
        // We use `Encoded` here because the GStreamer raw-formats list
        // doesn't enumerate MJPEG as a `video/x-raw` format — we still
        // hit the same code path: `caps_for` only Display-formats the
        // VideoFormat, it never inspects what kind of format it is.
        let caps = caps_for(fmt_in, VideoFormat::Encoded);

        let structure = caps.structure(0).unwrap();
        assert_eq!(structure.get::<i32>("width").unwrap(), 640);
        assert_eq!(structure.get::<i32>("height").unwrap(), 480);
        assert_eq!(
            structure.get::<Fraction>("framerate").unwrap(),
            Fraction::new(60, 1)
        );
    }

    /// `gstreamer::Caps::builder` requires the global registry to be
    /// initialised. Same `Once`-guard pattern as
    /// `controls::tests::ensure_gst_init` and `format::tests`.
    fn ensure_gst_init() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            gstreamer::init().expect("gstreamer::init() must succeed in tests");
        });
    }
}
