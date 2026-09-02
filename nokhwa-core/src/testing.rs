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

//! Mock backend implementations of the capability traits for use in tests.
//!
//! Enable with the `testing` feature. These types are intentionally minimal
//! and deterministic; they perform no I/O.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::buffer::Buffer;
use crate::error::NokhwaError;
use crate::traits::{
    CameraDevice, CameraEvent, EventPoll, EventSource, FrameSource, ShutterCapture,
};
use crate::types::{
    ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
    FrameFormat, KnownCameraControl, Resolution,
};

/// Build a deterministic [`CameraInfo`] for tests.
#[must_use]
pub fn mock_info(index: u32) -> CameraInfo {
    CameraInfo::new(
        "Mock Camera".to_string(),
        "mock camera for tests".to_string(),
        "mock".to_string(),
        false,
        CameraIndex::Index(index),
    )
}

/// Build a deterministic [`Buffer`] of the given shape. The payload is a
/// zero-filled vector sized to `w * h * bpp`, where `bpp` is a plausible
/// bytes-per-pixel for the chosen format.
///
/// The per-format `bpp` values here are intentionally coarse and meant only
/// for test fixtures — the real encoded byte counts (especially for MJPEG
/// and sub-sampled YUV formats) differ and should not be inferred from this
/// helper.
#[must_use]
pub fn mock_frame(width: u32, height: u32, format: FrameFormat) -> Buffer {
    let bpp = format.decoded_pixel_byte_width();
    let len = (width as usize) * (height as usize) * bpp;
    let data = vec![0u8; len];
    Buffer::from_vec(Resolution::new(width, height), data, format)
}

fn default_format() -> CameraFormat {
    CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30)
}

/// A simple continuous-frame mock backend.
pub struct MockFrameSource {
    info: CameraInfo,
    format: CameraFormat,
    is_open: bool,
    queue: VecDeque<Buffer>,
}

impl MockFrameSource {
    /// Create a new mock with an empty frame queue.
    #[must_use]
    pub fn new(index: u32) -> Self {
        Self {
            info: mock_info(index),
            format: default_format(),
            is_open: false,
            queue: VecDeque::new(),
        }
    }

    /// Push a frame onto the queue. `frame()` returns frames in FIFO order.
    pub fn push_frame(&mut self, frame: Buffer) {
        self.queue.push_back(frame);
    }
}

impl Default for MockFrameSource {
    fn default() -> Self {
        Self::new(0)
    }
}

impl CameraDevice for MockFrameSource {
    fn backend(&self) -> ApiBackend {
        ApiBackend::Browser
    }
    fn info(&self) -> &CameraInfo {
        &self.info
    }
    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        Ok(vec![])
    }
    fn set_control(
        &mut self,
        _id: KnownCameraControl,
        _value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl FrameSource for MockFrameSource {
    fn negotiated_format(&self) -> CameraFormat {
        self.format
    }
    fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        self.format = f;
        Ok(())
    }
    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        Ok(vec![self.format])
    }
    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        Ok(vec![self.format.format()])
    }

    fn open(&mut self) -> Result<(), NokhwaError> {
        self.is_open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.is_open
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        if let Some(buf) = self.queue.pop_front() {
            Ok(buf)
        } else {
            Err(NokhwaError::TimeoutError(Duration::ZERO))
        }
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        match self.queue.pop_front() {
            Some(buf) => Ok(Cow::Owned(buf.buffer().to_vec())),
            None => Err(NokhwaError::TimeoutError(Duration::ZERO)),
        }
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        self.is_open = false;
        Ok(())
    }
}

/// A simple shutter-capture mock backend.
pub struct MockShutter {
    info: CameraInfo,
    triggered: VecDeque<Buffer>,
    pending: VecDeque<Buffer>,
}

impl MockShutter {
    /// Create a new mock. `pictures` is the FIFO pool that each `trigger()`
    /// pulls from, enqueuing a picture for the next `take_picture`.
    #[must_use]
    pub fn new(pictures: Vec<Buffer>) -> Self {
        Self {
            info: mock_info(0),
            triggered: pictures.into(),
            pending: VecDeque::new(),
        }
    }
}

impl CameraDevice for MockShutter {
    fn backend(&self) -> ApiBackend {
        ApiBackend::Browser
    }
    fn info(&self) -> &CameraInfo {
        &self.info
    }
    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        Ok(vec![])
    }
    fn set_control(
        &mut self,
        _id: KnownCameraControl,
        _value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl ShutterCapture for MockShutter {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        if let Some(pic) = self.triggered.pop_front() {
            self.pending.push_back(pic);
        }
        Ok(())
    }
    fn take_picture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
        if let Some(pic) = self.pending.pop_front() {
            Ok(pic)
        } else {
            Err(NokhwaError::TimeoutError(timeout))
        }
    }
}

/// Hybrid mock that combines [`FrameSource`] and [`ShutterCapture`].
pub struct MockHybrid {
    frames: MockFrameSource,
    shutter: MockShutter,
}

impl MockHybrid {
    #[must_use]
    pub fn new(index: u32, pictures: Vec<Buffer>) -> Self {
        Self {
            frames: MockFrameSource::new(index),
            shutter: MockShutter::new(pictures),
        }
    }

    pub fn push_frame(&mut self, frame: Buffer) {
        self.frames.push_frame(frame);
    }
}

impl CameraDevice for MockHybrid {
    fn backend(&self) -> ApiBackend {
        self.frames.backend()
    }
    fn info(&self) -> &CameraInfo {
        self.frames.info()
    }
    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.frames.controls()
    }
    fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.frames.set_control(id, value)
    }
}

impl FrameSource for MockHybrid {
    fn negotiated_format(&self) -> CameraFormat {
        self.frames.negotiated_format()
    }
    fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        self.frames.set_format(f)
    }
    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        self.frames.compatible_formats()
    }
    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        self.frames.compatible_fourcc()
    }
    fn open(&mut self) -> Result<(), NokhwaError> {
        self.frames.open()
    }
    fn is_open(&self) -> bool {
        self.frames.is_open()
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.frames.frame()
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        self.frames.frame_raw()
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        self.frames.close()
    }
}

impl ShutterCapture for MockHybrid {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.shutter.trigger()
    }
    fn take_picture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
        self.shutter.take_picture(timeout)
    }
}

/// An [`EventPoll`] implementation backed by an [`std::sync::mpsc`] receiver.
pub struct MpscEventPoll {
    rx: Receiver<CameraEvent>,
}

impl MpscEventPoll {
    #[must_use]
    pub fn new(rx: Receiver<CameraEvent>) -> Self {
        Self { rx }
    }
}

impl EventPoll for MpscEventPoll {
    fn try_next(&mut self) -> Option<CameraEvent> {
        self.rx.try_recv().ok()
    }
    fn next_timeout(&mut self, d: Duration) -> Option<CameraEvent> {
        self.rx.recv_timeout(d).ok()
    }
}

/// A [`FrameSource`] + [`EventSource`] mock, useful for testing event delivery.
pub struct MockEventfulFrameSource {
    inner: MockFrameSource,
    poll: Option<Box<dyn EventPoll + Send>>,
}

impl MockEventfulFrameSource {
    /// Create a new mock that will hand out `poll` on the first (and only)
    /// call to [`EventSource::take_events`].
    #[must_use]
    pub fn new(index: u32, poll: Box<dyn EventPoll + Send>) -> Self {
        Self {
            inner: MockFrameSource::new(index),
            poll: Some(poll),
        }
    }

    pub fn push_frame(&mut self, frame: Buffer) {
        self.inner.push_frame(frame);
    }
}

impl CameraDevice for MockEventfulFrameSource {
    fn backend(&self) -> ApiBackend {
        self.inner.backend()
    }
    fn info(&self) -> &CameraInfo {
        self.inner.info()
    }
    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.inner.controls()
    }
    fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.inner.set_control(id, value)
    }
}

impl FrameSource for MockEventfulFrameSource {
    fn negotiated_format(&self) -> CameraFormat {
        self.inner.negotiated_format()
    }
    fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        self.inner.set_format(f)
    }
    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        self.inner.compatible_formats()
    }
    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        self.inner.compatible_fourcc()
    }
    fn open(&mut self) -> Result<(), NokhwaError> {
        self.inner.open()
    }
    fn is_open(&self) -> bool {
        self.inner.is_open()
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.inner.frame()
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        self.inner.frame_raw()
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        self.inner.close()
    }
}

impl EventSource for MockEventfulFrameSource {
    fn take_events(&mut self) -> Result<Box<dyn EventPoll + Send>, NokhwaError> {
        self.poll
            .take()
            .ok_or(NokhwaError::UnsupportedOperationError(ApiBackend::Browser))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn frame_source_returns_pushed_frames_in_order() {
        let mut src = MockFrameSource::new(0);
        assert!(!src.is_open());
        src.open().unwrap();
        assert!(src.is_open());
        src.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
        src.push_frame(mock_frame(8, 8, FrameFormat::YUYV));
        let a = src.frame().unwrap();
        let b = src.frame().unwrap();
        assert_eq!(a.resolution(), Resolution::new(4, 4));
        assert_eq!(b.resolution(), Resolution::new(8, 8));
        assert!(matches!(
            src.frame(),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
        src.close().unwrap();
        assert!(!src.is_open());
    }

    #[test]
    fn shutter_triggers_and_takes_pictures() {
        let pics = vec![
            mock_frame(2, 2, FrameFormat::MJPEG),
            mock_frame(3, 3, FrameFormat::MJPEG),
        ];
        let mut sh = MockShutter::new(pics);
        // `MockShutter::take_picture` (`testing.rs:206-212`) propagates
        // the caller-supplied timeout verbatim into `TimeoutError(_)`. A
        // regression that hard-coded a different `Duration` (e.g. always
        // `MAX`) would mislead downstream telemetry — pin the round-trip
        // exactly. Sibling tests at `:477,492,627` already pin in the
        // tighter shape; aligning this site keeps the file consistent.
        assert!(matches!(
            sh.take_picture(Duration::ZERO),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
        sh.trigger().unwrap();
        let p = sh.take_picture(Duration::from_millis(10)).unwrap();
        assert_eq!(p.resolution(), Resolution::new(2, 2));
    }

    // `MockShutter` uses a two-queue (`triggered → pending`) FIFO so that
    // `trigger()` and `take_picture()` are independent state moves —
    // exactly the contract a real shutter backend has. The single
    // `shutter_triggers_and_takes_pictures` test only exercises one
    // trigger+take, so a regression that swapped the queues, drained
    // them in LIFO order, or short-circuited the dance with a single
    // `VecDeque` would slip through. Pin the multi-step contract.
    #[test]
    fn mock_shutter_multi_trigger_drains_in_fifo_order() {
        let pics = vec![
            mock_frame(2, 2, FrameFormat::MJPEG),
            mock_frame(3, 3, FrameFormat::MJPEG),
            mock_frame(4, 4, FrameFormat::MJPEG),
        ];
        let mut sh = MockShutter::new(pics);
        sh.trigger().unwrap();
        sh.trigger().unwrap();
        sh.trigger().unwrap();
        let a = sh.take_picture(Duration::ZERO).unwrap();
        let b = sh.take_picture(Duration::ZERO).unwrap();
        let c = sh.take_picture(Duration::ZERO).unwrap();
        assert_eq!(a.resolution(), Resolution::new(2, 2));
        assert_eq!(b.resolution(), Resolution::new(3, 3));
        assert_eq!(c.resolution(), Resolution::new(4, 4));
        // After draining all three queued pictures, the next take_picture
        // must propagate the passed timeout (`Duration::ZERO`) verbatim
        // into `TimeoutError`. Pin the value to align with the
        // tighter shape already used at `:477,492,627`.
        assert!(matches!(
            sh.take_picture(Duration::ZERO),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
    }

    // `trigger()` on a `MockShutter` whose pool is empty is a silent
    // no-op (returns Ok, enqueues nothing). This matches a real
    // backend that accepted the command but had no picture ready. Pin
    // it so a regression that started returning `Err` on empty-pool
    // trigger (which would surface as spurious failures in any test
    // that ignores the picture pool) fails fast here.
    #[test]
    fn mock_shutter_trigger_on_empty_pool_is_silent_noop() {
        let mut sh = MockShutter::new(vec![]);
        sh.trigger().unwrap();
        sh.trigger().unwrap();
        assert!(matches!(
            sh.take_picture(Duration::ZERO),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
    }

    // `take_picture` on an empty `pending` queue must propagate the
    // exact `Duration` the caller passed in via `TimeoutError`. A
    // regression that hard-coded `Duration::ZERO` (or some other
    // sentinel) into the error would silently mislead callers about
    // how long they waited. Pin the round-trip.
    #[test]
    fn mock_shutter_take_picture_propagates_passed_timeout() {
        let mut sh = MockShutter::new(vec![]);
        let d = Duration::from_millis(123);
        assert!(matches!(
            sh.take_picture(d),
            Err(NokhwaError::TimeoutError(got)) if got == d
        ));
    }

    // `lock_ui` / `unlock_ui` are no-op `Ok` defaults on the
    // `ShutterCapture` trait — webcams have no UI to lock. Pin that
    // `MockShutter` (which doesn't override them) inherits the default
    // and that the `capture()` convenience routes through the full
    // `lock_ui → trigger → take_picture → unlock_ui` sequence. A
    // regression that gave `MockShutter` an explicit `lock_ui`
    // returning `Err` would break the `capture()` convenience for
    // every consumer relying on the trait default.
    #[test]
    fn mock_shutter_default_lock_unlock_ui_are_ok() {
        let mut sh = MockShutter::new(vec![mock_frame(2, 2, FrameFormat::MJPEG)]);
        sh.lock_ui().unwrap();
        sh.unlock_ui().unwrap();
        let pic = sh.capture(Duration::from_millis(10)).unwrap();
        assert_eq!(pic.resolution(), Resolution::new(2, 2));
    }

    // `MockHybrid` forwards `ShutterCapture` to its inner `MockShutter`
    // but does not override `lock_ui`/`unlock_ui`, so they resolve to
    // the trait defaults. Pin the same contract on the hybrid path so
    // `capture()` also works there — a regression where `MockHybrid`
    // grew an explicit `lock_ui` impl that errored would silently
    // break dual-capability tests using the convenience method.
    #[test]
    fn mock_hybrid_default_lock_unlock_ui_and_capture() {
        let mut h = MockHybrid::new(0, vec![mock_frame(5, 5, FrameFormat::MJPEG)]);
        h.lock_ui().unwrap();
        h.unlock_ui().unwrap();
        let pic = h.capture(Duration::from_millis(10)).unwrap();
        assert_eq!(pic.resolution(), Resolution::new(5, 5));
        assert_eq!(pic.source_frame_format(), FrameFormat::MJPEG);
    }

    // `EventPoll: Send` is a super-trait bound that lets pollers be
    // moved across thread boundaries (e.g. boxed into a worker thread
    // by `CameraRunner`). `MpscEventPoll` has to satisfy it for the
    // `Box<dyn EventPoll + Send>` path in `EventSource::take_events`
    // to even type-check. A refactor that introduced a non-`Send`
    // field (e.g. an `Rc`) would break that downstream boxing without
    // failing any current test directly. Pin via a generic shim — if
    // the bound regresses, this file fails to compile.
    fn assert_send<T: Send>() {}

    #[test]
    fn mpsc_event_poll_is_send() {
        assert_send::<MpscEventPoll>();
    }

    #[test]
    fn mpsc_poll_delivers_events() {
        let (tx, rx) = channel();
        let mut poll = MpscEventPoll::new(rx);
        assert!(poll.try_next().is_none());
        tx.send(CameraEvent::WillShutDown).unwrap();
        assert!(matches!(poll.try_next(), Some(CameraEvent::WillShutDown)));
        tx.send(CameraEvent::Disconnected).unwrap();
        assert!(matches!(
            poll.next_timeout(Duration::from_millis(50)),
            Some(CameraEvent::Disconnected)
        ));
        drop(tx);
        assert!(poll.next_timeout(Duration::from_millis(5)).is_none());
    }

    #[test]
    fn eventful_source_hands_out_poll_once() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let mut src = MockEventfulFrameSource::new(0, poll);
        assert!(src.take_events().is_ok());
        assert!(matches!(
            src.take_events(),
            Err(NokhwaError::UnsupportedOperationError(_))
        ));
    }

    #[test]
    fn mock_info_round_trip() {
        let info = mock_info(7);
        assert_eq!(info.index(), &CameraIndex::Index(7));
        assert_eq!(info.human_name(), "Mock Camera");
        assert_eq!(info.description(), "mock camera for tests");
        assert_eq!(info.misc(), "mock");
    }

    #[test]
    fn mock_frame_three_byte_formats_size_w_h_3() {
        for f in [
            FrameFormat::MJPEG,
            FrameFormat::YUYV,
            FrameFormat::RAWRGB,
            FrameFormat::RAWBGR,
            FrameFormat::NV12,
        ] {
            let buf = mock_frame(8, 4, f);
            assert_eq!(buf.resolution(), Resolution::new(8, 4));
            assert_eq!(buf.source_frame_format(), f);
            assert_eq!(buf.buffer().len(), 8 * 4 * 3, "format {f:?}");
        }
    }

    #[test]
    fn mock_frame_gray_size_w_h_1() {
        let buf = mock_frame(8, 4, FrameFormat::GRAY);
        assert_eq!(buf.resolution(), Resolution::new(8, 4));
        assert_eq!(buf.source_frame_format(), FrameFormat::GRAY);
        assert_eq!(buf.buffer().len(), 8 * 4);
    }

    #[test]
    fn mock_frame_zero_dimensions_yields_empty_buffer() {
        let buf = mock_frame(0, 0, FrameFormat::YUYV);
        assert_eq!(buf.buffer().len(), 0);
        assert_eq!(buf.resolution(), Resolution::new(0, 0));
    }

    // ─────────────── MockHybrid: dual-capability dispatch ───────────────

    #[test]
    fn mock_hybrid_frame_path_returns_pushed_frames_in_order() {
        let mut h = MockHybrid::new(0, vec![mock_frame(2, 2, FrameFormat::MJPEG)]);
        h.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
        h.push_frame(mock_frame(8, 8, FrameFormat::YUYV));
        h.open().unwrap();
        let a = h.frame().unwrap();
        let b = h.frame().unwrap();
        assert_eq!(a.resolution(), Resolution::new(4, 4));
        assert_eq!(b.resolution(), Resolution::new(8, 8));
        assert_eq!(a.source_frame_format(), FrameFormat::YUYV);
        assert!(matches!(
            h.frame(),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
    }

    #[test]
    fn mock_hybrid_shutter_path_independent_of_frame_queue() {
        let mut h = MockHybrid::new(0, vec![mock_frame(2, 2, FrameFormat::MJPEG)]);
        h.push_frame(mock_frame(8, 8, FrameFormat::YUYV));
        // Trigger the shutter, then take_picture: routes to inner MockShutter.
        // The queued frame stays in the frames queue (no cross-talk).
        h.trigger().unwrap();
        let pic = h.take_picture(Duration::from_millis(10)).unwrap();
        assert_eq!(pic.resolution(), Resolution::new(2, 2));
        assert_eq!(pic.source_frame_format(), FrameFormat::MJPEG);
        h.open().unwrap();
        let frame = h.frame().unwrap();
        assert_eq!(frame.resolution(), Resolution::new(8, 8));
        assert_eq!(frame.source_frame_format(), FrameFormat::YUYV);
    }

    #[test]
    fn mock_hybrid_take_picture_without_trigger_times_out() {
        let mut h = MockHybrid::new(0, vec![mock_frame(2, 2, FrameFormat::MJPEG)]);
        // `MockHybrid::take_picture` routes to the inner `MockShutter`,
        // which propagates the caller-supplied timeout verbatim. Pin the
        // round-trip — the sibling at `:625-628` already does, so align.
        assert!(matches!(
            h.take_picture(Duration::ZERO),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
    }

    #[test]
    fn mock_hybrid_open_close_state_routes_to_frame_source() {
        let mut h = MockHybrid::new(0, vec![]);
        assert!(!h.is_open());
        h.open().unwrap();
        assert!(h.is_open());
        h.close().unwrap();
        assert!(!h.is_open());
    }

    #[test]
    fn mock_hybrid_camera_device_metadata_routes_to_frame_source() {
        let h = MockHybrid::new(7, vec![]);
        assert_eq!(h.info().index(), &CameraIndex::Index(7));
        assert_eq!(h.backend(), MockFrameSource::new(0).backend());
        assert_eq!(h.controls().unwrap(), Vec::<CameraControl>::new());
    }

    // ─────── MockEventfulFrameSource FrameSource passthrough ───────
    //
    // `MockEventfulFrameSource` wraps a `MockFrameSource` and adds an
    // `EventSource` impl. Its `EventSource::take_events` is covered by
    // `eventful_source_hands_out_poll_once`, but every `FrameSource`
    // method on it is a thin forward to the inner mock — and a
    // regression where one of those forwards goes to the wrong field
    // (e.g. a copy-paste bug returning `Default::default()` instead
    // of `inner.negotiated_format()`) would slip through that single
    // existing test. These tests pin the passthrough contract on
    // each `FrameSource` method individually.

    #[test]
    fn mock_eventful_frame_source_open_close_routes_to_inner() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let mut src = MockEventfulFrameSource::new(0, poll);
        assert!(!src.is_open(), "starts closed");
        src.open().unwrap();
        assert!(src.is_open(), "open() flips inner.is_open");
        src.close().unwrap();
        assert!(!src.is_open(), "close() flips inner.is_open back");
    }

    #[test]
    fn mock_eventful_frame_source_push_frame_drains_via_frame() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let mut src = MockEventfulFrameSource::new(0, poll);
        src.open().unwrap();
        src.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
        src.push_frame(mock_frame(2, 2, FrameFormat::MJPEG));
        let first = src.frame().unwrap();
        assert_eq!(first.resolution(), Resolution::new(4, 4));
        assert_eq!(first.source_frame_format(), FrameFormat::YUYV);
        let second = src.frame().unwrap();
        assert_eq!(second.resolution(), Resolution::new(2, 2));
        assert_eq!(second.source_frame_format(), FrameFormat::MJPEG);
        // `MockEventfulFrameSource::frame` (`testing.rs:374-376`) routes
        // to the base `MockFrameSource::frame`, which emits
        // `TimeoutError(Duration::ZERO)` on an empty queue
        // (`testing.rs:145`). Pin the value — sibling at `:412` already
        // does, so align.
        assert!(matches!(
            src.frame(),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
    }

    #[test]
    fn mock_eventful_frame_source_set_and_negotiated_format() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let mut src = MockEventfulFrameSource::new(0, poll);
        // Default mirrors `MockFrameSource`'s default (640x480 YUYV @ 30).
        let default_fmt = src.negotiated_format();
        assert_eq!(default_fmt.resolution(), Resolution::new(640, 480));
        assert_eq!(default_fmt.format(), FrameFormat::YUYV);
        assert_eq!(default_fmt.frame_rate(), 30);

        let new_fmt = CameraFormat::new(Resolution::new(1920, 1080), FrameFormat::MJPEG, 60);
        src.set_format(new_fmt).unwrap();
        assert_eq!(src.negotiated_format(), new_fmt);
        assert_eq!(src.compatible_formats().unwrap(), vec![new_fmt]);
        assert_eq!(src.compatible_fourcc().unwrap(), vec![FrameFormat::MJPEG],);
    }

    #[test]
    fn mock_eventful_frame_source_camera_device_metadata_passthrough() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let src = MockEventfulFrameSource::new(11, poll);
        assert_eq!(src.info().index(), &CameraIndex::Index(11));
        assert_eq!(src.backend(), ApiBackend::Browser);
        assert_eq!(src.controls().unwrap(), Vec::<CameraControl>::new());
    }

    #[test]
    fn mock_eventful_frame_source_frame_raw_returns_pushed_bytes() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let mut src = MockEventfulFrameSource::new(0, poll);
        src.open().unwrap();
        let pushed = mock_frame(4, 4, FrameFormat::RAWRGB);
        let expected = pushed.buffer().to_vec();
        src.push_frame(pushed);
        let raw = src.frame_raw().unwrap();
        assert_eq!(&*raw, &expected[..]);
        // `MockEventfulFrameSource::frame_raw` (`testing.rs:377-378`)
        // routes to the base `MockFrameSource::frame_raw`, which emits
        // `TimeoutError(Duration::ZERO)` on an empty queue
        // (`testing.rs:151`). Pin the value — sibling at `:780` already
        // does, so align.
        assert!(matches!(
            src.frame_raw(),
            Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO
        ));
    }

    // The base `MockFrameSource::frame_raw` (`testing.rs:148-153`) is
    // exercised indirectly through `MockEventfulFrameSource`'s
    // passthrough, but not directly. A regression that, say, swapped
    // the `frame()` and `frame_raw()` bodies (returning a `Buffer`
    // wrapper inside a `Cow::Owned` of zero bytes, or stripping the
    // payload bytes) would still pass the eventful wrapper test
    // because that test relies on the same code path. Pin the base
    // contract directly: success returns owned bytes equal to the
    // pushed buffer, empty queue returns
    // `TimeoutError(Duration::ZERO)` exactly like `frame()`.
    #[test]
    fn mock_frame_source_frame_raw_returns_pushed_bytes_and_zero_timeout_on_empty() {
        let mut src = MockFrameSource::new(0);
        let pushed = mock_frame(2, 3, FrameFormat::RAWRGB);
        let expected = pushed.buffer().to_vec();
        src.push_frame(pushed);
        let raw = src.frame_raw().unwrap();
        assert_eq!(&*raw, &expected[..]);
        // Subsequent call on a now-empty queue must return
        // `TimeoutError(Duration::ZERO)` (line 151) — pin the exact
        // duration so a regression that surfaced a different sentinel
        // (`Duration::MAX`, an env-var override, etc.) would fail.
        match src.frame_raw() {
            Err(NokhwaError::TimeoutError(d)) => assert_eq!(d, Duration::ZERO),
            other => panic!("expected TimeoutError(0), got {other:?}"),
        }
    }

    // `MockFrameSource::compatible_formats` and `compatible_fourcc`
    // (`testing.rs:127-132`) intentionally echo back a single-element
    // vector containing only the currently-negotiated format. This
    // is what makes the mock useful for exercising format-negotiation
    // logic that picks "the only thing on offer". A regression that
    // returned `vec![]` (no options) or a wider list (every
    // `FrameFormat` variant) would fundamentally change the mock's
    // semantics. The eventful wrapper test already pins this through
    // the passthrough, but the base mock is its own user-facing
    // surface — pin it directly so a divergence between base and
    // wrapper would show up here.
    #[test]
    fn mock_frame_source_compatible_formats_echo_singleton() {
        let mut src = MockFrameSource::new(0);
        // Default format is 640x480 YUYV @ 30 (line 65).
        let default_fmt = src.negotiated_format();
        assert_eq!(src.compatible_formats().unwrap(), vec![default_fmt]);
        assert_eq!(src.compatible_fourcc().unwrap(), vec![FrameFormat::YUYV]);

        // After `set_format`, the singleton must echo the new format.
        let new_fmt = CameraFormat::new(Resolution::new(1920, 1080), FrameFormat::MJPEG, 60);
        src.set_format(new_fmt).unwrap();
        assert_eq!(src.compatible_formats().unwrap(), vec![new_fmt]);
        assert_eq!(src.compatible_fourcc().unwrap(), vec![FrameFormat::MJPEG]);
    }

    // `MockEventfulFrameSource::take_events` returns
    // `UnsupportedOperationError(ApiBackend::Browser)` on the
    // second call (`testing.rs:386-390`). The existing
    // `eventful_source_hands_out_poll_once` test uses
    // `matches!(_, Err(UnsupportedOperationError(_)))` and discards
    // the backend payload — a regression that hard-coded a different
    // backend (`ApiBackend::OpenCv`, `ApiBackend::Auto`,
    // `ApiBackend::Custom("...")`) would mislead error consumers
    // about which backend refused the second take and slip past the
    // existing assertion. Pin the exact backend variant.
    #[test]
    fn eventful_source_second_take_events_carries_browser_backend() {
        let (_tx, rx) = channel();
        let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
        let mut src = MockEventfulFrameSource::new(0, poll);
        src.take_events().unwrap();
        // `Result<Box<dyn EventPoll + Send>, NokhwaError>` doesn't
        // implement `Debug` (the trait object's vtable is opaque), so
        // use `Result::err()` to discard the unreachable `Ok` arm and
        // then assert the variant + payload directly.
        let err = src.take_events().err().expect("second take must fail");
        match err {
            NokhwaError::UnsupportedOperationError(b) => {
                assert_eq!(b, ApiBackend::Browser);
            },
            other => panic!("expected UnsupportedOperationError(Browser), got {other:?}"),
        }
    }
}
