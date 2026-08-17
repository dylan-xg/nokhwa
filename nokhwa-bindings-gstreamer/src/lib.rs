/*
 * Copyright 2022 l1npengtul <l1npengtul@protonmail.com> / The Nokhwa Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#![deny(clippy::pedantic)]
#![warn(clippy::all)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
// Matches the MSMF bindings crate: platform integration code casts
// small-integer pixel-format / fraction / CID values across i32 / u32
// / i64 / u32 boundaries constantly; `try_from` would swap real
// problems for a sea of boilerplate `.unwrap()`s. Keep the lints off
// at crate scope.
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
// Long fns in this crate are the pipeline-construction path in
// `PipelineHandle::start` (local + URL sources) and the open() routing
// in `lib.rs::new`. Splitting them by line count alone would harm
// readability more than it helps.
#![allow(clippy::too_many_lines)]
// `doc_markdown` fires on every unbackticked identifier (`GStreamer`,
// `AppSink`, `DeviceMonitor`, …). Keeping prose readable is worth more
// than enforcing backticks in every mention.
#![allow(clippy::doc_markdown)]

//! # nokhwa-bindings-gstreamer
//!
//! Cross-platform `GStreamer` bindings for `nokhwa`. Covers device
//! enumeration, streaming, format negotiation, controls (Linux), and URL
//! sources (`rtsp://` / `http://` / `file://`) via `uridecodebin`. Backend
//! parity with the native V4L / AVFoundation / MediaFoundation paths;
//! gaps are tracked in the root project `TODO.md`.
//!
//! This crate is consumed through `nokhwa` with feature `input-gstreamer`.
//! Do not depend on it directly.

#[cfg(all(feature = "backend", not(feature = "docs-only")))]
mod controls;
#[cfg(all(feature = "backend", not(feature = "docs-only")))]
mod format;
#[cfg(all(feature = "backend", not(feature = "docs-only")))]
mod pipeline;
#[cfg(all(feature = "backend", not(feature = "docs-only")))]
mod uri;

#[cfg(all(feature = "backend", not(feature = "docs-only")))]
mod internal {
    use crate::controls::{
        build_extra_controls, control_handle, list_controls, set_live_property, v4l2_cid_value,
        GstControlHandle,
    };
    use crate::pipeline::{
        compatible_formats as caps_for_device, compatible_fourcc as fourcc_for_device,
        ensure_gst_init, find_device, resolve_format, snapshot_video_devices, PipelineHandle,
    };
    use crate::uri::{compatible_fourcc_from_negotiated, UriPipelineHandle};
    use gstreamer::prelude::*;
    use gstreamer::Device;
    use nokhwa_core::{
        buffer::Buffer,
        error::NokhwaError,
        looks_like_url_scheme,
        traits::{CameraDevice, FrameSource},
        types::{
            ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
            FrameFormat, KnownCameraControl, RequestedFormat,
        },
    };
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    /// Enumerate video sources visible to the `GStreamer` device registry.
    ///
    /// Initialises `GStreamer` on first call (idempotent), creates a
    /// [`DeviceMonitor`] filtered to `Video/Source` with `video/x-raw`
    /// caps, starts the monitor, snapshots the device list, and stops
    /// the monitor. Each [`Device`](gstreamer::Device) becomes a
    /// [`CameraInfo`] with:
    /// - `human_name` from `device.display_name()`.
    /// - `description` from `device.device_class()` (e.g. `"Video/Source"`).
    /// - `misc` holds the display name as a stable re-identification
    ///   key for `GStreamerCaptureDevice::new()` to rediscover the
    ///   underlying [`Device`] across successive `DeviceMonitor`
    ///   invocations. Two cameras that share a display name fall back
    ///   to positional index.
    /// - `index` as a monotonic `CameraIndex::Index(n)`.
    pub fn query() -> Result<Vec<CameraInfo>, NokhwaError> {
        let devices = snapshot_video_devices()?;

        let mut cameras = Vec::with_capacity(devices.len());
        for (idx, dev) in devices.into_iter().enumerate() {
            let name = dev.display_name().to_string();
            let class = dev.device_class().to_string();
            cameras.push(CameraInfo::new(
                &name,
                &class,
                &name,
                CameraIndex::Index(u32::try_from(idx).unwrap_or(u32::MAX)),
            ));
        }
        Ok(cameras)
    }

    /// Local-device source metadata: what `DeviceMonitor` + `Device`
    /// gave us at `new()` time. Lives inside [`BackendSource::Local`].
    struct LocalSource {
        device: Device,
        formats: Vec<CameraFormat>,
        negotiated: CameraFormat,
        /// V4L2 CIDs to apply via `extra-controls` on the next
        /// pipeline open. Keyed by the CID name (e.g. `"zoom_absolute"`).
        /// Accumulates across `set_control` calls until the next
        /// `open()` flushes it into the source element.
        pending_extra_controls: BTreeMap<String, i64>,
    }

    /// URL-based source metadata: the URI we build a pipeline around.
    /// `negotiated` is populated after the first successful `open()`
    /// because URL streams don't advertise caps before we connect.
    /// This branch handles `rtsp://` / `http://` / `file://` etc.
    struct UriSource {
        uri: String,
        /// `None` until the first successful `open()` — no pre-flight
        /// probe API exists for URL streams short of fully connecting,
        /// so `compatible_formats()` and `negotiated_format()` return
        /// empty / default until we've actually opened.
        negotiated: Option<CameraFormat>,
    }

    enum BackendSource {
        Local(LocalSource),
        Uri(UriSource),
    }

    /// Runtime-active pipeline. The variant mirrors [`BackendSource`]
    /// so we can pattern-match for source-element access.
    enum ActivePipeline {
        Local(PipelineHandle),
        Uri(UriPipelineHandle),
    }

    impl ActivePipeline {
        fn source_element(&self) -> &gstreamer::Element {
            match self {
                Self::Local(p) => p.source(),
                Self::Uri(p) => p.source(),
            }
        }
        fn pull_frame(&self) -> Result<Buffer, NokhwaError> {
            match self {
                Self::Local(p) => p.pull_frame(),
                Self::Uri(p) => p.pull_frame(),
            }
        }
    }

    /// Cross-platform `GStreamer` capture device.
    ///
    /// Streaming uses a `source ! capsfilter ! videoconvert ! appsink`
    /// pipeline for local devices and a `uridecodebin ! videoconvert !
    /// appsink` pipeline for URL sources. The local-device source
    /// element is the one `Device::create_element()` hands us —
    /// `v4l2src` on Linux, `mfvideosrc` on Windows, `avfvideosrc` on
    /// macOS — so format enumeration and actual negotiation happen
    /// against the real device caps rather than a hardcoded element
    /// name. URL dispatch (`rtsp://` / `http://` / `https://` /
    /// `file://`, see `nokhwa_core::looks_like_url_scheme`) routes through
    /// `uridecodebin` regardless of platform. The two pipelines live
    /// in `BackendSource::Local` and `BackendSource::Uri`
    /// respectively.
    ///
    /// Controls are Linux-only: `v4l2src` exposes four `controllable`
    /// GObject properties (brightness / contrast / hue / saturation)
    /// that work at any pipeline state, plus the write-only
    /// `extra-controls` structure for the rest of the V4L2 CID
    /// namespace (exposure / zoom / focus / pan / tilt etc). Windows
    /// `mfvideosrc` / `ksvideosrc` and macOS `avfvideosrc` expose no
    /// camera-control properties — on those platforms `controls()`
    /// returns an empty list and `set_control()` errors. Users who
    /// need full control support on Windows / macOS should use the
    /// native `input-msmf` / `input-avfoundation` backends.
    ///
    /// `nokhwa::open()` dispatches `CameraIndex::Index(_)` to the
    /// platform native backend and URL-shaped `CameraIndex::String`s
    /// here.
    pub struct GStreamerCaptureDevice {
        info: CameraInfo,
        source: BackendSource,
        pipeline: Option<ActivePipeline>,
    }

    impl GStreamerCaptureDevice {
        pub fn new(index: &CameraIndex, cam_fmt: RequestedFormat) -> Result<Self, NokhwaError> {
            ensure_gst_init()?;

            // Branch 1: URL-like string → uridecodebin pipeline.
            if let CameraIndex::String(s) = index {
                if looks_like_url_scheme(s) {
                    let info = CameraInfo::new(s, "URL", s, index.clone());
                    return Ok(Self {
                        info,
                        source: BackendSource::Uri(UriSource {
                            uri: s.clone(),
                            negotiated: None,
                        }),
                        pipeline: None,
                    });
                }
            }

            // Branch 2: local device lookup via DeviceMonitor.
            let (display_name, positional) = match index {
                CameraIndex::Index(i) => (String::new(), *i),
                CameraIndex::String(s) => (s.clone(), 0),
            };
            let device = find_device(&display_name, positional)?;

            let formats = caps_for_device(&device);
            let negotiated = resolve_format(&formats, &cam_fmt)?;

            let name = device.display_name().to_string();
            let class = device.device_class().to_string();
            let info = CameraInfo::new(&name, &class, &name, index.clone());

            Ok(Self {
                info,
                source: BackendSource::Local(LocalSource {
                    device,
                    formats,
                    negotiated,
                    pending_extra_controls: BTreeMap::new(),
                }),
                pipeline: None,
            })
        }
    }

    impl CameraDevice for GStreamerCaptureDevice {
        fn backend(&self) -> ApiBackend {
            ApiBackend::GStreamer
        }

        fn info(&self) -> &CameraInfo {
            &self.info
        }

        fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
            // Without an open pipeline we have no source element to
            // introspect. Errors rather than returning `Ok(vec![])` so
            // the distinction between "no controls at all" and "ask
            // me again after `open()`" stays visible.
            let Some(pipeline) = &self.pipeline else {
                return Err(NokhwaError::get_property(
                    "controls",
                    "GStreamer controls() requires an open pipeline; call open() first",
                ));
            };
            // URL sources have an `uridecodebin` source element that
            // exposes none of the v4l2-style control properties;
            // `list_controls` returns an empty Vec for it, which is
            // the right answer ("there are no live controls on this
            // stream") rather than an error.
            Ok(list_controls(pipeline.source_element()))
        }

        fn set_control(
            &mut self,
            id: KnownCameraControl,
            value: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            let BackendSource::Local(local) = &mut self.source else {
                // URL-mode sources don't have controls — errors cleanly
                // rather than silently accepting a write that would
                // never be applied.
                return Err(NokhwaError::set_property(
                    id.to_string(),
                    value.to_string(),
                    "GStreamer URL-mode sources do not support controls",
                ));
            };
            let handle = control_handle(id).ok_or_else(|| {
                NokhwaError::set_property(
                    id.to_string(),
                    value.to_string(),
                    "KnownCameraControl::Other is not mapped by the GStreamer backend",
                )
            })?;
            match handle {
                GstControlHandle::Property(name) => {
                    let Some(ActivePipeline::Local(pipeline)) = &self.pipeline else {
                        return Err(NokhwaError::set_property(
                            name.to_string(),
                            value.to_string(),
                            "pipeline not open; open() before set_control for live controls",
                        ));
                    };
                    set_live_property(pipeline.source(), name, &value)
                },
                GstControlHandle::V4l2Cid(cid) => {
                    // Stage it in `pending_extra_controls` — it takes
                    // effect on the next `open()` via v4l2src's
                    // `extra-controls` property. If the pipeline is
                    // already open we tear it down and restart so the
                    // change lands immediately; matches what the MSMF
                    // backend does for non-live property writes.
                    //
                    // The staged insert happens unconditionally so the
                    // value is present on the next successful `open()`
                    // even if the restart below fails.
                    let int_value = v4l2_cid_value(cid, &value)?;
                    local
                        .pending_extra_controls
                        .insert(cid.to_string(), int_value);
                    if matches!(self.pipeline, Some(ActivePipeline::Local(_))) {
                        // Build the replacement pipeline before tearing
                        // down the current one. On failure the old
                        // pipeline keeps running and the device stays
                        // in a usable streaming state.
                        let new_pipeline = PipelineHandle::start(
                            &local.device,
                            local.negotiated,
                            build_extra_controls(&local.pending_extra_controls)?,
                        )?;
                        self.pipeline = Some(ActivePipeline::Local(new_pipeline));
                    }
                    Ok(())
                },
            }
        }
    }

    impl FrameSource for GStreamerCaptureDevice {
        fn negotiated_format(&self) -> CameraFormat {
            match &self.source {
                BackendSource::Local(l) => l.negotiated,
                // URL mode: prefer the live pipeline's current
                // format (set from the first sample) over the
                // not-yet-known default.
                BackendSource::Uri(u) => match (&self.pipeline, u.negotiated) {
                    (Some(ActivePipeline::Uri(p)), _) => p.format(),
                    (_, Some(f)) => f,
                    (_, None) => CameraFormat::default(),
                },
            }
        }

        fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
            let BackendSource::Local(local) = &mut self.source else {
                return Err(NokhwaError::set_property(
                    "CameraFormat",
                    format!("{f:?}"),
                    "GStreamer URL-mode sources negotiate format from the stream; \
                            set_format is not meaningful",
                ));
            };
            if !local.formats.contains(&f) {
                return Err(NokhwaError::set_property(
                    "CameraFormat",
                    format!("{f:?}"),
                    "not in the device's compatible format list",
                ));
            }
            let was_open = matches!(self.pipeline, Some(ActivePipeline::Local(_)));
            if was_open {
                // Build the new pipeline first. Only commit `negotiated`
                // and replace `self.pipeline` after the start succeeds so
                // that a failure leaves the device in its prior state
                // (old format, old pipeline still running).
                let new_pipeline = PipelineHandle::start(
                    &local.device,
                    f,
                    build_extra_controls(&local.pending_extra_controls)?,
                )?;
                local.negotiated = f;
                self.pipeline = Some(ActivePipeline::Local(new_pipeline));
            } else {
                // Pipeline not open: just record the desired format;
                // it takes effect on the next `open()`.
                local.negotiated = f;
                self.pipeline = None;
            }
            Ok(())
        }

        fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
            match &self.source {
                BackendSource::Local(l) => Ok(l.formats.clone()),
                BackendSource::Uri(u) => {
                    // Once opened, we know the one format the stream
                    // delivers. Before that, an empty list is the most
                    // honest answer.
                    Ok(u.negotiated.map(|f| vec![f]).unwrap_or_default())
                },
            }
        }

        fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
            match &self.source {
                BackendSource::Local(l) => Ok(fourcc_for_device(&l.formats)),
                BackendSource::Uri(u) => Ok(u
                    .negotiated
                    .map(compatible_fourcc_from_negotiated)
                    .unwrap_or_default()),
            }
        }

        fn open(&mut self) -> Result<(), NokhwaError> {
            if self.pipeline.is_some() {
                return Ok(());
            }
            match &mut self.source {
                BackendSource::Local(local) => {
                    self.pipeline = Some(ActivePipeline::Local(PipelineHandle::start(
                        &local.device,
                        local.negotiated,
                        build_extra_controls(&local.pending_extra_controls)?,
                    )?));
                },
                BackendSource::Uri(uri) => {
                    let handle = UriPipelineHandle::start(&uri.uri)?;
                    uri.negotiated = Some(handle.format());
                    self.pipeline = Some(ActivePipeline::Uri(handle));
                },
            }
            Ok(())
        }

        fn is_open(&self) -> bool {
            self.pipeline.is_some()
        }

        fn frame(&mut self) -> Result<Buffer, NokhwaError> {
            match &self.pipeline {
                Some(p) => p.pull_frame(),
                None => Err(NokhwaError::ReadFrameError {
                    message: "pipeline not open — call open() first".to_string(),
                    format: Some(self.negotiated_format().format()),
                }),
            }
        }

        fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
            // Cow::Owned wrap of the pulled frame's bytes. A borrowed
            // variant isn't safe here because the AppSink sample's
            // memory mapping is scoped to the pull call.
            let buf = self.frame()?;
            Ok(Cow::Owned(buf.buffer().to_vec()))
        }

        fn close(&mut self) -> Result<(), NokhwaError> {
            self.pipeline = None;
            Ok(())
        }
    }
}

#[cfg(any(not(feature = "backend"), feature = "docs-only"))]
mod internal {
    use nokhwa_core::{
        buffer::Buffer,
        error::NokhwaError,
        traits::{CameraDevice, FrameSource},
        types::{
            ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
            FrameFormat, KnownCameraControl, RequestedFormat,
        },
    };
    use std::borrow::Cow;

    /// Stub [`query`] for builds without the `backend` feature. The
    /// real implementation requires `gstreamer-rs` (and a system
    /// `GStreamer` install); consumers enable it via the top-level
    /// `input-gstreamer` feature on the `nokhwa` crate.
    pub fn query() -> Result<Vec<CameraInfo>, NokhwaError> {
        Err(NokhwaError::NotImplementedError(
            "GStreamer backend not compiled in (enable feature `input-gstreamer` on the `nokhwa` \
             crate)"
                .to_string(),
        ))
    }

    /// Stub [`GStreamerCaptureDevice`] for builds without the `backend`
    /// feature. Every method errors with
    /// [`NokhwaError::NotImplementedError`].
    pub struct GStreamerCaptureDevice;

    #[allow(unused_variables)]
    impl GStreamerCaptureDevice {
        pub fn new(index: &CameraIndex, cam_fmt: RequestedFormat) -> Result<Self, NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "GStreamer backend not compiled in".to_string(),
            ))
        }
    }

    #[allow(unused_variables)]
    impl CameraDevice for GStreamerCaptureDevice {
        fn backend(&self) -> ApiBackend {
            ApiBackend::GStreamer
        }

        fn info(&self) -> &CameraInfo {
            unreachable!("GStreamer stub: GStreamerCaptureDevice::new always fails")
        }

        fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn set_control(
            &mut self,
            id: KnownCameraControl,
            value: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }
    }

    #[allow(unused_variables)]
    impl FrameSource for GStreamerCaptureDevice {
        fn negotiated_format(&self) -> CameraFormat {
            CameraFormat::default()
        }

        fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn open(&mut self) -> Result<(), NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn is_open(&self) -> bool {
            false
        }

        fn frame(&mut self) -> Result<Buffer, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }

        fn close(&mut self) -> Result<(), NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::GStreamer,
            ))
        }
    }
}

pub use internal::*;

#[cfg(all(test, not(feature = "backend")))]
mod stub_tests {
    use super::internal::{query, GStreamerCaptureDevice};
    use nokhwa_core::error::NokhwaError;
    use nokhwa_core::format_types::Mjpeg;
    use nokhwa_core::traits::{CameraDevice, FrameSource};
    use nokhwa_core::types::{
        ApiBackend, CameraFormat, CameraIndex, ControlValueSetter, FrameFormat, KnownCameraControl,
        RequestedFormat, RequestedFormatType, Resolution,
    };

    // The GStreamer stub uses a *different* error shape from the V4L /
    // MSMF stubs: `query()` and `GStreamerCaptureDevice::new` return
    // `NotImplementedError` ("backend not compiled in"), but the trait-
    // method paths return `UnsupportedOperationError(ApiBackend::
    // GStreamer)` because the dispatcher in `src/backends/capture/mod.rs`
    // wires this stub in even on backend-less builds (unlike V4L / MSMF
    // which are cfg-gated out). Pin both error shapes so a future
    // refactor that "unifies" them silently breaks the dispatcher's
    // expectations.

    fn assert_not_implemented(err: &NokhwaError) {
        assert!(
            matches!(err, NokhwaError::NotImplementedError(_)),
            "expected NotImplementedError, got {err:?}",
        );
    }

    fn assert_unsupported_for_gstreamer(err: &NokhwaError) {
        assert!(
            matches!(
                err,
                NokhwaError::UnsupportedOperationError(ApiBackend::GStreamer)
            ),
            "expected UnsupportedOperationError(GStreamer), got {err:?}",
        );
    }

    /// Verifies the stub path compiles and returns a `NotImplementedError`
    /// carrying the canonical actionable payload that tells the user
    /// which feature flag to flip. Previously this only checked
    /// `Display::contains("GStreamer")`, which would still pass if a
    /// future refactor dropped the `enable feature input-gstreamer`
    /// hint or rephrased "not compiled in" → "disabled" — both real
    /// regressions for users who hit the stub by accident on
    /// non-Linux builds. Pin both the variant payload and the full
    /// `Display` form via the canonical wrapper string.
    #[test]
    fn stub_query_errors_cleanly() {
        let err = query().expect_err("stub query() must error");
        assert_not_implemented(&err);
        let payload = match &err {
            NokhwaError::NotImplementedError(p) => p.clone(),
            other => panic!("expected NotImplementedError, got {other:?}"),
        };
        assert_eq!(
            payload,
            "GStreamer backend not compiled in (enable feature `input-gstreamer` on the `nokhwa` \
             crate)"
        );
        // `NokhwaError`'s `Display` wraps the payload as
        // "This operation is not implemented yet: {payload}". Pin the
        // wrapper too so a future refactor that re-routes through a
        // different variant constructor (and silently drops the
        // wrapper prefix) is caught.
        assert_eq!(
            format!("{err}"),
            format!("This operation is not implemented yet: {payload}"),
        );
    }

    #[test]
    fn stub_new_errors_with_not_implemented() {
        // `GStreamerCaptureDevice` does not implement `Debug`.
        match GStreamerCaptureDevice::new(
            &CameraIndex::Index(0),
            RequestedFormat::new::<Mjpeg>(RequestedFormatType::AbsoluteHighestFrameRate),
        ) {
            Err(err) => assert_not_implemented(&err),
            Ok(_) => panic!("stub `new` must always error without backend feature"),
        }
    }

    #[test]
    fn stub_backend_reports_gstreamer() {
        let dev = GStreamerCaptureDevice;
        assert_eq!(dev.backend(), ApiBackend::GStreamer);
    }

    #[test]
    fn stub_camera_device_methods_return_unsupported() {
        let mut dev = GStreamerCaptureDevice;
        assert_unsupported_for_gstreamer(&dev.controls().expect_err("stub controls() must error"));
        assert_unsupported_for_gstreamer(
            &dev.set_control(
                KnownCameraControl::Brightness,
                ControlValueSetter::Integer(0),
            )
            .expect_err("stub set_control() must error"),
        );
    }

    #[test]
    fn stub_frame_source_methods_return_unsupported() {
        let mut dev = GStreamerCaptureDevice;
        assert_unsupported_for_gstreamer(
            &dev.set_format(CameraFormat::new(
                Resolution::new(640, 480),
                FrameFormat::MJPEG,
                30,
            ))
            .expect_err("stub set_format() must error"),
        );
        assert_unsupported_for_gstreamer(
            &dev.compatible_formats()
                .expect_err("stub compatible_formats() must error"),
        );
        assert_unsupported_for_gstreamer(
            &dev.compatible_fourcc()
                .expect_err("stub compatible_fourcc() must error"),
        );
        assert_unsupported_for_gstreamer(&dev.open().expect_err("stub open() must error"));
        assert_unsupported_for_gstreamer(&dev.frame().expect_err("stub frame() must error"));
        assert_unsupported_for_gstreamer(
            &dev.frame_raw().expect_err("stub frame_raw() must error"),
        );
        assert_unsupported_for_gstreamer(&dev.close().expect_err("stub close() must error"));
    }

    #[test]
    fn stub_is_open_reports_false() {
        let dev = GStreamerCaptureDevice;
        assert!(!dev.is_open());
    }

    /// Unlike V4L / MSMF stubs (which `unreachable!()` on
    /// `negotiated_format`), the GStreamer stub returns
    /// `CameraFormat::default()` — pin that explicitly because some
    /// callers in `src/session.rs` invoke `negotiated_format()` even on
    /// stub paths.
    #[test]
    fn stub_negotiated_format_returns_default_not_panic() {
        let dev = GStreamerCaptureDevice;
        assert_eq!(dev.negotiated_format(), CameraFormat::default());
    }

    /// `info()` is the one method that still panics on this stub (see
    /// `unreachable!("GStreamer stub: GStreamerCaptureDevice::new always
    /// fails")`). It's only reachable if a caller fabricates a
    /// `GStreamerCaptureDevice` directly bypassing `new` — pin that
    /// path so a future refactor doesn't silently switch it to a
    /// default `CameraInfo` (which would be a worse failure mode:
    /// callers would treat a stub device as a real camera).
    #[test]
    #[should_panic(expected = "GStreamer stub: GStreamerCaptureDevice::new always fails")]
    fn stub_info_panics() {
        let dev = GStreamerCaptureDevice;
        let _info = dev.info();
    }
}

#[cfg(all(test, feature = "backend", not(feature = "docs-only")))]
mod backend_tests {
    use super::internal::query;

    /// Smoke test with the real GStreamer backend. Must not panic.
    /// Accepts both `Ok(vec)` and `Err(_)` because CI runners may
    /// have GStreamer installed with zero video-source plugins
    /// registered, in which case the monitor returns an empty list,
    /// while a sandboxed runner without `/dev/video*` access may
    /// surface `gstreamer::init()` errors.
    #[test]
    fn query_does_not_panic() {
        match query() {
            Ok(cameras) => {
                eprintln!("gstreamer::query() -> {} source(s)", cameras.len());
                for cam in &cameras {
                    eprintln!("  {} | {}", cam.human_name(), cam.description());
                }
            },
            Err(e) => eprintln!("gstreamer::query() errored (accepted): {e}"),
        }
    }
}
