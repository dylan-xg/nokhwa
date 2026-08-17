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

//! Layer 2: [`open`], [`OpenedCamera`], and the per-capability
//! wrapper types (`StreamCamera`, `ShutterCamera`, `HybridCamera`).
//!
//! These types replace the pre-0.13 `Camera` / `CallbackCamera` structs.
//! Each wrapper delegates to a boxed backend implementing the appropriate
//! capability traits from [`nokhwa_core::traits`]. Backends are registered
//! via the [`nokhwa_backend!`](crate::nokhwa_backend) macro, which implements the
//! `#[doc(hidden)]` [`AnyDevice`] trait for the backend and exposes its capability
//! bits so [`OpenedCamera::from_device`] can dispatch to the right variant.

use std::borrow::Cow;
use std::time::Duration;

use nokhwa_core::buffer::Buffer;
use nokhwa_core::error::NokhwaError;
use nokhwa_core::traits::{CameraDevice, EventPoll, FrameSource, ShutterCapture};
use nokhwa_core::types::{
    ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
    FrameFormat, KnownCameraControl,
};

/// Capability bit: backend implements [`FrameSource`].
#[doc(hidden)]
pub const CAP_FRAME: u32 = 1 << 0;
/// Capability bit: backend implements [`ShutterCapture`].
#[doc(hidden)]
pub const CAP_SHUTTER: u32 = 1 << 1;
/// Capability bit: backend implements [`EventSource`].
#[doc(hidden)]
pub const CAP_EVENT: u32 = 1 << 2;

/// Erased-backend trait with capability bits and owned downcasts.
///
/// Implemented via the [`nokhwa_backend!`] macro. Not intended to be used
/// or implemented directly; it is `pub` only so the macro expansion
/// (which may be in downstream crates) can see it.
#[doc(hidden)]
pub trait AnyDevice: Send {
    /// Bitwise-OR of `CAP_*` bits for capabilities this backend implements.
    fn capabilities(&self) -> u32;

    /// Consume self and produce a boxed [`FrameSource`]. Only called when
    /// `capabilities() & CAP_FRAME != 0`.
    fn into_frame_source(self: Box<Self>) -> Box<dyn FrameSource + Send>;

    /// Consume self and produce a boxed [`ShutterCapture`]. Only called when
    /// `capabilities() & CAP_SHUTTER != 0`.
    fn into_shutter(self: Box<Self>) -> Box<dyn ShutterCapture + Send>;

    /// Consume self and produce a boxed hybrid (both frame + shutter).
    /// Only called when both bits are present.
    fn into_hybrid(self: Box<Self>) -> Box<dyn HybridBackend + Send>;

    /// Take the event poller, if this backend is an [`EventSource`].
    fn take_events(&mut self) -> Option<Result<Box<dyn EventPoll + Send>, NokhwaError>>;
}

/// Trait object combining [`FrameSource`] and [`ShutterCapture`] for
/// [`HybridCamera`].
#[doc(hidden)]
pub trait HybridBackend: FrameSource + ShutterCapture + Send {}
impl<T: FrameSource + ShutterCapture + Send> HybridBackend for T {}

/// A request to open a camera via [`open`].
///
/// The `Copy` derive relies on every field being `Copy`. If a future field is
/// non-`Copy`, drop the `Copy` derive (keep `Clone`) and pass by reference
/// at the API boundary instead.
#[derive(Debug, Clone, Copy)]
pub struct OpenRequest {
    format: Option<CameraFormat>,
}

impl OpenRequest {
    /// Open with the backend's default format negotiation.
    #[must_use]
    pub fn any() -> Self {
        Self { format: None }
    }

    /// Open and request a specific [`CameraFormat`].
    #[must_use]
    pub fn with_format(format: CameraFormat) -> Self {
        Self {
            format: Some(format),
        }
    }

    /// The requested format, if any.
    #[must_use]
    pub fn format(&self) -> Option<CameraFormat> {
        self.format
    }
}

impl Default for OpenRequest {
    fn default() -> Self {
        Self::any()
    }
}

/// Open a camera or URL source.
///
/// ## Routing
///
/// **`CameraIndex::String` with a URL-like scheme** (`rtsp://`, `rtsps://`,
/// `rtmp://`, `rtmps://`, `http://`, `https://`, `file://`, `srt://`,
/// `udp://`, `tcp://`) is routed to the `GStreamer` backend via
/// `uridecodebin`. This path requires the `input-gstreamer` feature and a
/// system `GStreamer` install. If the feature is not enabled, the call falls
/// through to the native backend which will return an error for URI strings
/// it cannot open.
///
/// **All other indices** (numeric `CameraIndex::Index`, or
/// `CameraIndex::String` values that do not look like a URI) are dispatched
/// at compile time via `cfg` to the V4L2 backend on Linux, the
/// `AVFoundation` backend on macOS/iOS, or the Media Foundation backend on
/// Windows (subject to the corresponding `input-*` feature being enabled).
/// If none of the native `input-*` features are enabled for the current
/// target the call returns an error at runtime rather than failing at
/// compile time.
///
/// Note: this function acquires the device. It is unrelated to the
/// `open()` method on [`StreamCamera`] / [`HybridCamera`], which starts
/// the frame stream on an already-acquired device.
///
/// # Errors
/// Returns [`NokhwaError`] if no native backend is available for the
/// current platform/feature configuration, or if the underlying backend
/// fails to open the device.
pub fn open(index: CameraIndex, req: OpenRequest) -> Result<OpenedCamera, NokhwaError> {
    use nokhwa_core::types::{color_frame_formats, RequestedFormat, RequestedFormatType};

    let requested = match req.format {
        Some(fmt) => {
            RequestedFormat::with_formats(RequestedFormatType::Exact(fmt), color_frame_formats())
        },
        None => RequestedFormat::with_formats(
            RequestedFormatType::AbsoluteHighestResolution,
            color_frame_formats(),
        ),
    };

    // URL-like strings short-circuit to the GStreamer backend. Native
    // backends can't open `rtsp://` / `http://` / `file://` URIs. This
    // check runs before the native branches so plain local cameras keep
    // their fast path but URL strings route correctly even on hosts
    // with a native backend compiled in.
    #[cfg(feature = "input-gstreamer")]
    if let CameraIndex::String(s) = &index {
        if nokhwa_core::looks_like_url_scheme(s) {
            use crate::backends::capture::GStreamerCaptureDevice;
            let dev = GStreamerCaptureDevice::new(&index, requested)?;
            return Ok(OpenedCamera::from_device(Box::new(dev)));
        }
    }

    #[cfg(all(target_os = "linux", feature = "input-v4l"))]
    {
        use nokhwa_bindings_linux_v4l::V4LCaptureDevice;
        let dev = V4LCaptureDevice::new(&index, requested)?;
        return Ok(OpenedCamera::from_device(Box::new(dev)));
    }
    #[cfg(all(
        any(target_os = "macos", target_os = "ios"),
        feature = "input-avfoundation"
    ))]
    {
        use nokhwa_bindings_macos_avfoundation::AVFoundationCaptureDevice;
        let dev = AVFoundationCaptureDevice::new(&index, requested)?;
        return Ok(OpenedCamera::from_device(Box::new(dev)));
    }
    #[cfg(all(target_os = "windows", feature = "input-msmf"))]
    {
        use nokhwa_bindings_windows_msmf::MediaFoundationCaptureDevice;
        let dev = MediaFoundationCaptureDevice::new(&index, requested)?;
        return Ok(OpenedCamera::from_device(Box::new(dev)));
    }
    // Cross-platform GStreamer fallback — used when no native backend
    // matched the target / feature configuration above. The `allow`
    // covers builds that compile both a native backend AND
    // `input-gstreamer`, where every native branch above returns
    // unconditionally and this block is statically unreachable.
    #[cfg(feature = "input-gstreamer")]
    #[allow(unreachable_code)]
    {
        use crate::backends::capture::GStreamerCaptureDevice;
        let dev = GStreamerCaptureDevice::new(&index, requested)?;
        return Ok(OpenedCamera::from_device(Box::new(dev)));
    }
    #[allow(unreachable_code)]
    {
        let _ = (index, requested);
        Err(NokhwaError::general(
            "no native backend available for this platform/feature configuration",
        ))
    }
}

/// An opened camera, dispatched by backend capability.
pub enum OpenedCamera {
    /// Stream-only backend (e.g. a webcam).
    Stream(StreamCamera),
    /// Shutter-only backend (e.g. a tethered DSLR without live view).
    Shutter(ShutterCamera),
    /// Hybrid backend providing both streaming and still-capture surfaces.
    Hybrid(HybridCamera),
}

impl OpenedCamera {
    /// Wrap a backend into the appropriate variant based on its capability
    /// bits.
    ///
    /// # Panics
    /// Panics if the backend advertises neither `CAP_FRAME` nor `CAP_SHUTTER`.
    #[doc(hidden)]
    #[must_use]
    pub fn from_device(device: Box<dyn AnyDevice>) -> Self {
        let caps = device.capabilities();
        let has_frame = caps & CAP_FRAME != 0;
        let has_shutter = caps & CAP_SHUTTER != 0;
        match (has_frame, has_shutter) {
            (true, true) => OpenedCamera::Hybrid(HybridCamera::from_device(device)),
            (true, false) => OpenedCamera::Stream(StreamCamera::from_device(device)),
            (false, true) => OpenedCamera::Shutter(ShutterCamera::from_device(device)),
            (false, false) => panic!(
                "nokhwa_backend!: device advertises no capabilities (neither FrameSource \
                 nor ShutterCapture)"
            ),
        }
    }
}

// ─────────────────────────── StreamCamera ─────────────────────────────

/// Wrapper for streaming-capable backends.
pub struct StreamCamera {
    inner: Box<dyn FrameSource + Send>,
}

impl StreamCamera {
    /// Build from a raw [`AnyDevice`] box. Used by [`OpenedCamera::from_device`]
    /// and directly by tests.
    ///
    /// # Panics
    /// Panics if the backend does not advertise `CAP_FRAME`.
    #[doc(hidden)]
    #[must_use]
    pub fn from_device(device: Box<dyn AnyDevice>) -> Self {
        assert!(
            device.capabilities() & CAP_FRAME != 0,
            "StreamCamera requires a FrameSource-capable backend"
        );
        Self {
            inner: device.into_frame_source(),
        }
    }

    // ── CameraDevice pass-through ────────────────────────────────
    #[must_use]
    pub fn backend(&self) -> ApiBackend {
        self.inner.backend()
    }
    #[must_use]
    pub fn info(&self) -> &CameraInfo {
        self.inner.info()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.inner.controls()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.inner.set_control(id, value)
    }

    // ── FrameSource pass-through ─────────────────────────────────
    #[must_use]
    pub fn negotiated_format(&self) -> CameraFormat {
        self.inner.negotiated_format()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        self.inner.set_format(f)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        self.inner.compatible_formats()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        self.inner.compatible_fourcc()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn open(&mut self) -> Result<(), NokhwaError> {
        self.inner.open()
    }
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.inner.is_open()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.inner.frame()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        self.inner.frame_raw()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn close(&mut self) -> Result<(), NokhwaError> {
        self.inner.close()
    }
}

// ─────────────────────────── ShutterCamera ────────────────────────────

/// Wrapper for shutter-capture-only backends.
pub struct ShutterCamera {
    inner: Box<dyn ShutterCapture + Send>,
}

impl ShutterCamera {
    /// Build from a raw [`AnyDevice`] box.
    ///
    /// # Panics
    /// Panics if the backend does not advertise `CAP_SHUTTER`.
    #[doc(hidden)]
    #[must_use]
    pub fn from_device(device: Box<dyn AnyDevice>) -> Self {
        assert!(
            device.capabilities() & CAP_SHUTTER != 0,
            "ShutterCamera requires a ShutterCapture-capable backend"
        );
        Self {
            inner: device.into_shutter(),
        }
    }

    #[must_use]
    pub fn backend(&self) -> ApiBackend {
        self.inner.backend()
    }
    #[must_use]
    pub fn info(&self) -> &CameraInfo {
        self.inner.info()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.inner.controls()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.inner.set_control(id, value)
    }

    /// # Errors
    /// Propagates the backend's error.
    pub fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.inner.trigger()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn take_picture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
        self.inner.take_picture(timeout)
    }
    /// Locks the camera's physical UI controls. See [`ShutterCapture::lock_ui`].
    /// # Errors
    /// Propagates the backend's error.
    pub fn lock_ui(&mut self) -> Result<(), NokhwaError> {
        self.inner.lock_ui()
    }
    /// Releases the UI lock. See [`ShutterCapture::unlock_ui`].
    /// # Errors
    /// Propagates the backend's error.
    pub fn unlock_ui(&mut self) -> Result<(), NokhwaError> {
        self.inner.unlock_ui()
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn capture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
        self.inner.capture(timeout)
    }
}

// ─────────────────────────── HybridCamera ─────────────────────────────

/// Wrapper for backends that implement both [`FrameSource`] and
/// [`ShutterCapture`].
pub struct HybridCamera {
    inner: Box<dyn HybridBackend + Send>,
    events: Option<Result<Box<dyn EventPoll + Send>, NokhwaError>>,
    event_source: bool,
}

impl HybridCamera {
    /// Build from a raw [`AnyDevice`] box.
    ///
    /// # Panics
    /// Panics if the backend does not advertise both `CAP_FRAME` and `CAP_SHUTTER`.
    #[doc(hidden)]
    #[must_use]
    pub fn from_device(mut device: Box<dyn AnyDevice>) -> Self {
        let caps = device.capabilities();
        assert!(
            caps & CAP_FRAME != 0 && caps & CAP_SHUTTER != 0,
            "HybridCamera requires both FrameSource and ShutterCapture"
        );
        let event_source = caps & CAP_EVENT != 0;
        let events = if event_source {
            match device.take_events() {
                Some(Ok(p)) => Some(Ok(p)),
                Some(Err(e)) => {
                    #[cfg(feature = "logging")]
                    log::warn!("HybridCamera: failed to take event poller: {e}");
                    #[cfg(not(feature = "logging"))]
                    let _ = e;
                    None
                },
                None => None,
            }
        } else {
            None
        };
        Self {
            inner: device.into_hybrid(),
            events,
            event_source,
        }
    }

    #[must_use]
    pub fn backend(&self) -> ApiBackend {
        CameraDevice::backend(&*self.inner)
    }
    #[must_use]
    pub fn info(&self) -> &CameraInfo {
        CameraDevice::info(&*self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        CameraDevice::controls(&*self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        CameraDevice::set_control(&mut *self.inner, id, value)
    }

    // FrameSource surface.
    #[must_use]
    pub fn negotiated_format(&self) -> CameraFormat {
        FrameSource::negotiated_format(&*self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        FrameSource::set_format(&mut *self.inner, f)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        FrameSource::compatible_formats(&mut *self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        FrameSource::compatible_fourcc(&mut *self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn open(&mut self) -> Result<(), NokhwaError> {
        FrameSource::open(&mut *self.inner)
    }
    #[must_use]
    pub fn is_open(&self) -> bool {
        FrameSource::is_open(&*self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        FrameSource::frame(&mut *self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        FrameSource::frame_raw(&mut *self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn close(&mut self) -> Result<(), NokhwaError> {
        FrameSource::close(&mut *self.inner)
    }

    // ShutterCapture surface.
    /// # Errors
    /// Propagates the backend's error.
    pub fn trigger(&mut self) -> Result<(), NokhwaError> {
        ShutterCapture::trigger(&mut *self.inner)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn take_picture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
        ShutterCapture::take_picture(&mut *self.inner, timeout)
    }
    /// # Errors
    /// Propagates the backend's error.
    pub fn capture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
        ShutterCapture::capture(&mut *self.inner, timeout)
    }

    /// Take the event poller, if this backend advertised [`EventSource`](nokhwa_core::traits::EventSource).
    ///
    /// Returns `None` on subsequent calls, for non-event backends, and when
    /// the backend's initial event-poll construction failed (that error is
    /// logged via `log::warn!` when the `logging` feature is enabled).
    ///
    /// The inner `Result` is always `Ok(_)` in 0.14 — init failures are
    /// normalised to `None` above. The shape is preserved for
    /// forward-compatibility with backends that may produce a poll lazily.
    pub fn take_events(&mut self) -> Option<Result<Box<dyn EventPoll + Send>, NokhwaError>> {
        if !self.event_source {
            return None;
        }
        // Invariant: `from_device` normalises `Some(Err(_))` to `None`, so the
        // only variants reachable here are `None` and `Some(Ok(_))`.
        debug_assert!(matches!(self.events, None | Some(Ok(_))));
        self.events.take()
    }
}

// ───────────────────────── nokhwa_backend! macro ──────────────────────

/// Internal: expand to a `into_frame_source` body that casts when possible,
/// or `unreachable!()` when the backend does not implement `FrameSource`.
#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_into_frame {
    ($ty:ty; has_frame) => {
        fn into_frame_source(
            self: ::std::boxed::Box<Self>,
        ) -> ::std::boxed::Box<dyn ::nokhwa_core::traits::FrameSource + ::std::marker::Send> {
            self
        }
    };
    ($ty:ty; no_frame) => {
        fn into_frame_source(
            self: ::std::boxed::Box<Self>,
        ) -> ::std::boxed::Box<dyn ::nokhwa_core::traits::FrameSource + ::std::marker::Send> {
            unreachable!(
                "nokhwa_backend!: {} does not implement FrameSource",
                ::std::stringify!($ty)
            )
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_into_shutter {
    ($ty:ty; has_shutter) => {
        fn into_shutter(
            self: ::std::boxed::Box<Self>,
        ) -> ::std::boxed::Box<dyn ::nokhwa_core::traits::ShutterCapture + ::std::marker::Send> {
            self
        }
    };
    ($ty:ty; no_shutter) => {
        fn into_shutter(
            self: ::std::boxed::Box<Self>,
        ) -> ::std::boxed::Box<dyn ::nokhwa_core::traits::ShutterCapture + ::std::marker::Send> {
            unreachable!(
                "nokhwa_backend!: {} does not implement ShutterCapture",
                ::std::stringify!($ty)
            )
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_into_hybrid {
    ($ty:ty; has_both) => {
        fn into_hybrid(
            self: ::std::boxed::Box<Self>,
        ) -> ::std::boxed::Box<dyn $crate::session::HybridBackend + ::std::marker::Send> {
            self
        }
    };
    ($ty:ty; no_both) => {
        fn into_hybrid(
            self: ::std::boxed::Box<Self>,
        ) -> ::std::boxed::Box<dyn $crate::session::HybridBackend + ::std::marker::Send> {
            unreachable!(
                "nokhwa_backend!: {} is not a hybrid backend",
                ::std::stringify!($ty)
            )
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_take_events {
    ($ty:ty; has_event) => {
        fn take_events(
            &mut self,
        ) -> ::std::option::Option<
            ::std::result::Result<
                ::std::boxed::Box<dyn ::nokhwa_core::traits::EventPoll + ::std::marker::Send>,
                ::nokhwa_core::error::NokhwaError,
            >,
        > {
            ::std::option::Option::Some(<Self as ::nokhwa_core::traits::EventSource>::take_events(
                self,
            ))
        }
    };
    ($ty:ty; no_event) => {
        fn take_events(
            &mut self,
        ) -> ::std::option::Option<
            ::std::result::Result<
                ::std::boxed::Box<dyn ::nokhwa_core::traits::EventPoll + ::std::marker::Send>,
                ::nokhwa_core::error::NokhwaError,
            >,
        > {
            ::std::option::Option::None
        }
    };
}

/// Register a backend type with the `nokhwa` Layer 2 session machinery.
///
/// The macro implements the (private) [`AnyDevice`] trait, which lets
/// [`OpenedCamera::from_device`] and the per-capability wrapper types
/// pick up the backend without requiring compile-time knowledge of its
/// concrete type.
///
/// ```ignore
/// use nokhwa::nokhwa_backend;
/// nokhwa_backend!(MyBackend: FrameSource);
/// nokhwa_backend!(MyDslr: ShutterCapture);
/// nokhwa_backend!(MyHybrid: FrameSource, ShutterCapture, EventSource);
/// ```
#[macro_export]
macro_rules! nokhwa_backend {
    ($ty:ty : $($cap:ident),+ $(,)?) => {
        $crate::__nokhwa_backend_scan!(
            @scan
            ty=($ty)
            f=no s=no e=no
            rest=( $($cap)+ )
        );
    };
}

/// Token-muncher: consume capability tokens one at a time, flip flags,
/// then emit the trait impl once the list is empty.
#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_backend_scan {
    // Recognise FrameSource.
    (@scan ty=($ty:ty) f=$_f:ident s=$s:ident e=$e:ident rest=( FrameSource $($rest:ident)* )) => {
        $crate::__nokhwa_backend_scan!(@scan ty=($ty) f=yes s=$s e=$e rest=( $($rest)* ));
    };
    // Recognise ShutterCapture.
    (@scan ty=($ty:ty) f=$f:ident s=$_s:ident e=$e:ident rest=( ShutterCapture $($rest:ident)* )) => {
        $crate::__nokhwa_backend_scan!(@scan ty=($ty) f=$f s=yes e=$e rest=( $($rest)* ));
    };
    // Recognise EventSource.
    (@scan ty=($ty:ty) f=$f:ident s=$s:ident e=$_e:ident rest=( EventSource $($rest:ident)* )) => {
        $crate::__nokhwa_backend_scan!(@scan ty=($ty) f=$f s=$s e=yes rest=( $($rest)* ));
    };
    // Done — emit the impl.
    (@scan ty=($ty:ty) f=$f:ident s=$s:ident e=$e:ident rest=( )) => {
        impl $crate::session::AnyDevice for $ty {
            fn capabilities(&self) -> u32 {
                let mut caps: u32 = 0;
                $crate::__nokhwa_add_bit!(caps, $f, $crate::session::CAP_FRAME);
                $crate::__nokhwa_add_bit!(caps, $s, $crate::session::CAP_SHUTTER);
                $crate::__nokhwa_add_bit!(caps, $e, $crate::session::CAP_EVENT);
                caps
            }
            $crate::__nokhwa_into_frame_pick!($ty, $f);
            $crate::__nokhwa_into_shutter_pick!($ty, $s);
            $crate::__nokhwa_into_hybrid_pick!($ty, $f, $s);
            $crate::__nokhwa_take_events_pick!($ty, $e);
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_add_bit {
    ($caps:ident, yes, $bit:expr) => {
        $caps |= $bit;
    };
    ($caps:ident, no, $bit:expr) => {};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_into_frame_pick {
    ($ty:ty, yes) => { $crate::__nokhwa_into_frame!($ty; has_frame); };
    ($ty:ty, no)  => { $crate::__nokhwa_into_frame!($ty; no_frame); };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_into_shutter_pick {
    ($ty:ty, yes) => { $crate::__nokhwa_into_shutter!($ty; has_shutter); };
    ($ty:ty, no)  => { $crate::__nokhwa_into_shutter!($ty; no_shutter); };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_into_hybrid_pick {
    ($ty:ty, yes, yes) => { $crate::__nokhwa_into_hybrid!($ty; has_both); };
    ($ty:ty, $f:ident, $s:ident) => { $crate::__nokhwa_into_hybrid!($ty; no_both); };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nokhwa_take_events_pick {
    ($ty:ty, yes) => { $crate::__nokhwa_take_events!($ty; has_event); };
    ($ty:ty, no)  => { $crate::__nokhwa_take_events!($ty; no_event); };
}

// NOTE: the `nokhwa_backend!` macros reference `::nokhwa_core` directly by
// absolute crate name AND use `$crate::session::{AnyDevice, HybridBackend,
// CAP_*}` from this crate on expansion. Downstream users of the macro therefore
// need **both** `nokhwa` and `nokhwa-core` as dependencies. The hidden items
// are `#[doc(hidden)] pub` so the expansion can see them; do not rely on them
// directly in application code.

#[cfg(test)]
mod uri_scheme_tests {
    use nokhwa_core::looks_like_url_scheme;

    #[test]
    fn detects_all_known_schemes() {
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
    fn rejects_non_uri_inputs() {
        for s in [
            "",
            "/dev/video0",
            "video0",
            "Logitech BRIO",
            "C:\\Users\\dave\\video.mp4",
            "ftp://example.com/file", // intentionally not in the list
            "ws://example.com",
            "data:text/plain,hello",
            ":colon-prefixed",
            "rtsp",      // missing `://`
            "http:/foo", // single slash
        ] {
            assert!(!looks_like_url_scheme(s), "expected non-URL: {s}");
        }
    }

    #[test]
    fn detection_is_case_insensitive() {
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
        for unsupported in ["ftp://", "ws://", "wss://", "data:", "mms://"] {
            assert!(
                !looks_like_url_scheme(unsupported),
                "{unsupported} unexpectedly recognised"
            );
        }
    }
}

#[cfg(test)]
mod open_request_tests {
    use super::OpenRequest;
    use nokhwa_core::types::{CameraFormat, FrameFormat, Resolution};

    /// `OpenRequest::any()` produces a request with no specific
    /// format. `open()` translates `format == None` to
    /// `RequestedFormatType::AbsoluteHighestResolution` — a
    /// regression where `any()` accidentally pinned a default format
    /// would silently override the highest-resolution negotiation
    /// path that every default caller relies on.
    #[test]
    fn any_has_no_format() {
        assert_eq!(OpenRequest::any().format(), None);
    }

    /// `OpenRequest::with_format(fmt)` round-trips through `format()`.
    /// Pins the constructor against a regression that drops the
    /// supplied format on the floor; every nontrivial open path goes
    /// through this constructor when the caller wants a specific
    /// resolution / framerate.
    #[test]
    fn with_format_round_trips() {
        let fmt = CameraFormat::new(Resolution::new(1920, 1080), FrameFormat::MJPEG, 30);
        let req = OpenRequest::with_format(fmt);
        assert_eq!(req.format(), Some(fmt));
    }

    /// `Default::default()` for `OpenRequest` must match
    /// `OpenRequest::any()`. Documented contract — derive consumers
    /// (e.g. struct fields with `#[derive(Default)]`) silently get
    /// the "no specific format" path. A divergence (e.g. someone
    /// adding a different default later) would change behaviour for
    /// every call site that uses the derive.
    #[test]
    fn default_equals_any() {
        assert_eq!(OpenRequest::default().format(), OpenRequest::any().format());
    }

    /// `OpenRequest` is `Copy` per its doc note. Round-trip it by
    /// value through a binding and confirm the original is still
    /// usable. A regression where someone adds a non-`Copy` field
    /// would force dropping the `Copy` derive; this catches the
    /// change at the unit-test layer rather than at every call site
    /// that passes the request by value.
    #[test]
    fn copy_semantics_preserved() {
        fn take_by_value(_r: OpenRequest) {}
        let fmt = CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30);
        let req = OpenRequest::with_format(fmt);
        take_by_value(req);
        assert_eq!(req.format(), Some(fmt), "OpenRequest must still be Copy");
    }
}

/// Pin the ABI shape of the capability bits used by the
/// `nokhwa_backend!` macro and the `OpenedCamera` dispatcher.
///
/// `CAP_FRAME` / `CAP_SHUTTER` / `CAP_EVENT` are `#[doc(hidden)] pub
/// const u32`, but they are observable across crate boundaries — the
/// macro expansion in any backend crate (including downstream
/// consumers that implement custom backends) ORs these constants
/// into a single `u32` capability mask, and `OpenedCamera::from_device`
/// branches on `caps & CAP_* != 0`. Changing any of these values
/// silently breaks the dispatch logic for already-compiled backend
/// crates, so the unit tests pin: (1) every constant is non-zero;
/// (2) every constant is a single-bit mask (power of two); (3) the
/// three masks are pairwise distinct; and (4) the OR of all three
/// fits in the low byte (room for ~5 more capability bits before the
/// macro logic needs to widen to `u64`).
#[cfg(test)]
mod capability_bits_tests {
    use super::{CAP_EVENT, CAP_FRAME, CAP_SHUTTER};

    #[test]
    fn all_caps_are_nonzero() {
        assert_ne!(CAP_FRAME, 0);
        assert_ne!(CAP_SHUTTER, 0);
        assert_ne!(CAP_EVENT, 0);
    }

    #[test]
    fn each_cap_is_single_bit() {
        for (name, bit) in [
            ("CAP_FRAME", CAP_FRAME),
            ("CAP_SHUTTER", CAP_SHUTTER),
            ("CAP_EVENT", CAP_EVENT),
        ] {
            assert_eq!(
                bit.count_ones(),
                1,
                "{name} must be a single-bit mask, got 0b{bit:b}"
            );
        }
    }

    #[test]
    fn caps_are_pairwise_distinct() {
        assert_ne!(CAP_FRAME, CAP_SHUTTER);
        assert_ne!(CAP_FRAME, CAP_EVENT);
        assert_ne!(CAP_SHUTTER, CAP_EVENT);
    }

    #[test]
    fn cap_union_fits_in_low_byte() {
        // Headroom check: today the OR of all three caps occupies
        // 3 bits in the low byte. If a future capability addition
        // pushes past 8 bits, the macro logic must widen to u64
        // (or some other shape) and this test forces the discussion.
        let union = CAP_FRAME | CAP_SHUTTER | CAP_EVENT;
        assert!(
            union <= 0xFF,
            "capability mask union 0b{union:b} no longer fits in the low byte"
        );
    }
}
