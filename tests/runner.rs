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

#![cfg(feature = "runner")]

//! Integration tests for [`nokhwa::runner`].

use std::borrow::Cow;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use nokhwa::nokhwa_backend;
use nokhwa::{CameraRunner, OpenedCamera, Overflow, RunnerConfig};
use nokhwa_core::buffer::Buffer;
use nokhwa_core::error::NokhwaError;
use nokhwa_core::testing::{mock_frame, MockFrameSource, MockHybrid, MockShutter, MpscEventPoll};
use nokhwa_core::traits::{
    CameraDevice, CameraEvent, EventPoll, EventSource, FrameSource, ShutterCapture,
};
use nokhwa_core::types::{
    ApiBackend, CameraControl, CameraFormat, CameraInfo, ControlValueSetter, FrameFormat,
    KnownCameraControl,
};

// ─────────────── local newtype wrappers (orphan-rule shim) ────────────

struct FrameOnly(MockFrameSource);

impl CameraDevice for FrameOnly {
    fn backend(&self) -> ApiBackend {
        self.0.backend()
    }
    fn info(&self) -> &CameraInfo {
        self.0.info()
    }
    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.0.controls()
    }
    fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.0.set_control(id, value)
    }
}
impl FrameSource for FrameOnly {
    fn negotiated_format(&self) -> CameraFormat {
        self.0.negotiated_format()
    }
    fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        self.0.set_format(f)
    }
    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        self.0.compatible_formats()
    }
    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        self.0.compatible_fourcc()
    }
    fn open(&mut self) -> Result<(), NokhwaError> {
        self.0.open()
    }
    fn is_open(&self) -> bool {
        self.0.is_open()
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.0.frame()
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        self.0.frame_raw()
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        self.0.close()
    }
}

struct ShutterOnly(MockShutter);

impl CameraDevice for ShutterOnly {
    fn backend(&self) -> ApiBackend {
        self.0.backend()
    }
    fn info(&self) -> &CameraInfo {
        self.0.info()
    }
    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.0.controls()
    }
    fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.0.set_control(id, value)
    }
}
impl ShutterCapture for ShutterOnly {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.0.trigger()
    }
    fn take_picture(&mut self, t: Duration) -> Result<Buffer, NokhwaError> {
        self.0.take_picture(t)
    }
}

/// Hybrid with events — MockHybrid + an mpsc event sender the test holds.
struct HybridWithEvents {
    inner: MockHybrid,
    poll: Option<Box<dyn EventPoll + Send>>,
}

impl HybridWithEvents {
    fn new(inner: MockHybrid, poll: Box<dyn EventPoll + Send>) -> Self {
        Self {
            inner,
            poll: Some(poll),
        }
    }
}

impl CameraDevice for HybridWithEvents {
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
impl FrameSource for HybridWithEvents {
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
impl ShutterCapture for HybridWithEvents {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.inner.trigger()
    }
    fn take_picture(&mut self, t: Duration) -> Result<Buffer, NokhwaError> {
        self.inner.take_picture(t)
    }
}
impl EventSource for HybridWithEvents {
    fn take_events(&mut self) -> Result<Box<dyn EventPoll + Send>, NokhwaError> {
        self.poll
            .take()
            .ok_or(NokhwaError::UnsupportedOperationError(ApiBackend::Browser))
    }
}

nokhwa_backend!(FrameOnly: FrameSource);
nokhwa_backend!(ShutterOnly: ShutterCapture);
nokhwa_backend!(HybridWithEvents: FrameSource, ShutterCapture, EventSource);

fn make_frame_only() -> FrameOnly {
    let mut s = MockFrameSource::new(0);
    // Push a handful of frames so the runner has something to deliver.
    for _ in 0..8 {
        s.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
    }
    FrameOnly(s)
}

fn make_shutter_only() -> ShutterOnly {
    ShutterOnly(MockShutter::new(vec![
        mock_frame(4, 4, FrameFormat::MJPEG),
        mock_frame(4, 4, FrameFormat::MJPEG),
    ]))
}

fn make_hybrid_with_events() -> (HybridWithEvents, Sender<CameraEvent>) {
    let mut h = MockHybrid::new(0, vec![mock_frame(4, 4, FrameFormat::MJPEG)]);
    for _ in 0..8 {
        h.push_frame(mock_frame(8, 8, FrameFormat::YUYV));
    }
    let (tx, rx) = channel();
    let poll: Box<dyn EventPoll + Send> = Box::new(MpscEventPoll::new(rx));
    (HybridWithEvents::new(h, poll), tx)
}

#[test]
fn runner_stream_delivers_frames() {
    let opened = OpenedCamera::from_device(Box::new(make_frame_only()));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    let rx = runner.frames().expect("stream runner has frames channel");
    let _buf = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("frame timed out");
    assert!(runner.pictures().is_none());
}

#[test]
fn runner_shutter_delivers_pictures_on_trigger() {
    let opened = OpenedCamera::from_device(Box::new(make_shutter_only()));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner.trigger().unwrap();
    let rx = runner
        .pictures()
        .expect("shutter runner has pictures channel");
    let _buf = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("picture timed out");
    assert!(runner.frames().is_none());
}

#[test]
fn runner_hybrid_delivers_both_and_events() {
    let (hybrid, tx) = make_hybrid_with_events();
    tx.send(CameraEvent::Disconnected).unwrap();
    let opened = OpenedCamera::from_device(Box::new(hybrid));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    let _ = runner
        .frames()
        .unwrap()
        .recv_timeout(Duration::from_millis(500))
        .unwrap();
    runner.trigger().unwrap();
    let _ = runner
        .pictures()
        .unwrap()
        .recv_timeout(Duration::from_millis(500))
        .unwrap();
    let _ = runner
        .events()
        .unwrap()
        .recv_timeout(Duration::from_millis(500))
        .unwrap();
}

#[test]
fn runner_drop_joins_thread() {
    let opened = OpenedCamera::from_device(Box::new(make_frame_only()));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    drop(runner);
}

/// M10: verify `RunnerConfig::shutter_timeout` is actually forwarded to
/// `ShutterCapture::take_picture`. An instrumented shutter records the
/// `timeout` passed to it and the runner is spawned with a custom value.
#[test]
fn runner_shutter_timeout_is_forwarded() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    struct TimeoutProbe {
        info: CameraInfo,
        observed_ms: Arc<AtomicU64>,
    }

    impl CameraDevice for TimeoutProbe {
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
            _v: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            Ok(())
        }
    }

    impl ShutterCapture for TimeoutProbe {
        fn trigger(&mut self) -> Result<(), NokhwaError> {
            Ok(())
        }
        fn take_picture(&mut self, timeout: Duration) -> Result<Buffer, NokhwaError> {
            self.observed_ms
                .store(timeout.as_millis() as u64, Ordering::SeqCst);
            Ok(mock_frame(4, 4, FrameFormat::MJPEG))
        }
    }

    nokhwa_backend!(TimeoutProbe: ShutterCapture);

    let observed = Arc::new(AtomicU64::new(0));
    let probe = TimeoutProbe {
        info: CameraInfo::new(
            "probe".to_string(),
            "probe".to_string(),
            "probe".to_string(),
            false,
            nokhwa_core::types::CameraIndex::Index(0),
        ),
        observed_ms: Arc::clone(&observed),
    };
    let opened = OpenedCamera::from_device(Box::new(probe));
    let cfg = RunnerConfig {
        shutter_timeout: Duration::from_millis(1234),
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();
    runner.trigger().unwrap();
    let _ = runner
        .pictures()
        .expect("shutter runner has pictures channel")
        .recv_timeout(Duration::from_secs(1))
        .expect("picture timed out");
    assert_eq!(observed.load(Ordering::SeqCst), 1234);
}

// ──────────────────── public-API default-value pins ───────────────────

#[test]
fn overflow_default_is_drop_newest() {
    assert_eq!(Overflow::default(), Overflow::DropNewest);
}

#[test]
fn overflow_derives_copy_eq() {
    let a = Overflow::DropOldest;
    let b = a;
    assert_eq!(a, b);
    assert_ne!(Overflow::DropNewest, Overflow::Block);
}

// `Overflow` derives `Debug`, so renaming a variant in `src/runner.rs:46-63`
// would silently change the diagnostic string emitted by panic messages,
// `dbg!()`, and any user-facing log line. Pin the literal variant names
// here so a rename forces a deliberate, reviewer-visible test update.
#[test]
fn overflow_debug_variant_names() {
    assert_eq!(format!("{:?}", Overflow::DropNewest), "DropNewest");
    assert_eq!(format!("{:?}", Overflow::DropOldest), "DropOldest");
    assert_eq!(format!("{:?}", Overflow::Block), "Block");
}

#[test]
fn runner_config_default_pins_field_values() {
    let cfg = RunnerConfig::default();
    assert_eq!(cfg.poll_interval, Duration::from_millis(10));
    assert_eq!(cfg.event_tick, Duration::from_millis(50));
    assert_eq!(cfg.shutter_timeout, Duration::from_secs(5));
    assert_eq!(cfg.frames_capacity, 4);
    assert_eq!(cfg.pictures_capacity, 8);
    assert_eq!(cfg.events_capacity, 32);
    assert_eq!(cfg.overflow, Overflow::DropNewest);
}

#[test]
fn runner_config_is_copy() {
    let cfg = RunnerConfig::default();
    let copied = cfg;
    assert_eq!(cfg.frames_capacity, copied.frames_capacity);
}

// ──────────────────── stop() and take_*() coverage ────────────────────

#[test]
fn runner_stop_returns_ok_on_stream_backend() {
    let opened = OpenedCamera::from_device(Box::new(make_frame_only()));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner.stop().unwrap();
}

#[test]
fn runner_stop_returns_ok_on_shutter_backend() {
    let opened = OpenedCamera::from_device(Box::new(make_shutter_only()));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner.stop().unwrap();
}

#[test]
fn runner_stop_returns_ok_on_hybrid_backend() {
    let (hybrid, _tx) = make_hybrid_with_events();
    let opened = OpenedCamera::from_device(Box::new(hybrid));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner.stop().unwrap();
}

#[test]
fn runner_take_frames_idempotent_on_stream() {
    let opened = OpenedCamera::from_device(Box::new(make_frame_only()));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    let rx = runner
        .take_frames()
        .expect("first take should yield receiver");
    assert!(runner.take_frames().is_none(), "second take must be None");
    assert!(
        runner.frames().is_none(),
        "frames() must be None after take_frames()"
    );
    let _buf = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("owned receiver still delivers frames");
}

#[test]
fn runner_take_pictures_idempotent_on_shutter() {
    let opened = OpenedCamera::from_device(Box::new(make_shutter_only()));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    let rx = runner
        .take_pictures()
        .expect("first take should yield receiver");
    assert!(runner.take_pictures().is_none(), "second take must be None");
    assert!(
        runner.pictures().is_none(),
        "pictures() must be None after take_pictures()"
    );
    runner.trigger().unwrap();
    let _buf = rx
        .recv_timeout(Duration::from_millis(500))
        .expect("owned receiver still delivers pictures after trigger");
}

#[test]
fn runner_take_pictures_none_on_stream_backend() {
    let opened = OpenedCamera::from_device(Box::new(make_frame_only()));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    assert!(runner.take_pictures().is_none());
    assert!(runner.take_events().is_none());
}

#[test]
fn runner_take_frames_none_on_shutter_backend() {
    let opened = OpenedCamera::from_device(Box::new(make_shutter_only()));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    assert!(runner.take_frames().is_none());
    assert!(runner.take_events().is_none());
}

#[test]
fn runner_take_events_yields_receiver_on_event_hybrid() {
    let (hybrid, tx) = make_hybrid_with_events();
    tx.send(CameraEvent::Disconnected).unwrap();
    let opened = OpenedCamera::from_device(Box::new(hybrid));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    let events_rx = runner
        .take_events()
        .expect("hybrid with EventSource should yield events receiver");
    assert!(runner.take_events().is_none(), "second take must be None");
    assert!(
        runner.events().is_none(),
        "events() must be None after take_events()"
    );
    let _ev = events_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("owned events receiver still delivers");
}

// ─────────────────── Overflow policy: E2E through CameraRunner ────────
//
// `make_channel(capacity, policy)` is unit-tested directly in
// `src/runner.rs`, but the full path from `CameraRunner::spawn(opened,
// RunnerConfig { overflow: ..., frames_capacity: ..., .. })` through
// the worker thread to the user-facing `Receiver<Buffer>` was never
// exercised end-to-end. A regression that hard-coded a single policy
// in `spawn`, swapped a pair of `frames_capacity` / `events_capacity`
// arguments, or forgot to wire the relay thread for `DropOldest`
// would silently degrade the policy without any test failing.
//
// These two tests pin the spawn → policy → user channel wiring with
// a sequence-number tag in the first byte of each frame so we can
// distinguish dropped vs. delivered without relying on raw counts.

/// Build a `MockFrameSource` whose Nth pushed frame has its first
/// data byte set to N (truncated to u8). Used to verify ordering
/// invariants without the test depending on exact frame counts.
fn make_sequenced_frame_source(count: u8) -> FrameOnly {
    let mut s = MockFrameSource::new(0);
    for n in 0..count {
        // 4×4 GRAY → 16 bytes; bpp=1 so the first byte is observable.
        let mut buf = mock_frame(4, 4, FrameFormat::GRAY).buffer().to_vec();
        buf[0] = n;
        s.push_frame(Buffer::from_vec(
            nokhwa_core::types::Resolution::new(4, 4),
            buf,
            FrameFormat::GRAY,
        ));
    }
    FrameOnly(s)
}

/// `Overflow::Block` end-to-end: with `frames_capacity = 1` and a
/// finite frame queue, the worker must block on a full channel
/// rather than drop frames. Pushing 4 sequenced frames and draining
/// after the source has stalled (errored on empty queue) must yield
/// exactly those 4 frames in order. A regression that swapped Block
/// for DropOldest / DropNewest would lose frames; a regression that
/// dropped Block to a no-op would fail to produce them at all.
#[test]
fn runner_overflow_block_delivers_every_frame_in_order() {
    let opened = OpenedCamera::from_device(Box::new(make_sequenced_frame_source(4)));
    let cfg = RunnerConfig {
        frames_capacity: 1,
        overflow: Overflow::Block,
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();
    let rx = runner.frames().expect("stream runner has frames channel");

    let mut received = Vec::new();
    // Pull until the source stalls. Each `recv_timeout` window must
    // be long enough to cover the runner's `poll_interval` (10ms
    // default) plus channel send latency.
    while let Ok(buf) = rx.recv_timeout(Duration::from_millis(500)) {
        received.push(buf.buffer()[0]);
        if received.len() == 4 {
            // Drained the entire pushed sequence; subsequent recv
            // would just time out on the stalled (empty-queue) source.
            break;
        }
    }

    assert_eq!(
        received,
        vec![0, 1, 2, 3],
        "Block must deliver every produced frame in order; got {received:?}"
    );
}

/// `Overflow::DropOldest` end-to-end: the relay thread is wired only
/// when this policy is selected. Pin two invariants — (a) frames
/// arrive in order (DropOldest never reorders), and (b) when the
/// source produces faster than we drain, the *last* frame produced
/// (highest sequence number) must always be observable, because
/// DropOldest evicts the front of the buffer to make room.
///
/// The test uses 16 finite sequenced frames (range fits in u8) and a
/// `frames_capacity = 1` channel. We don't drain during production —
/// we sleep long enough for the runner to walk the entire queue —
/// then drain everything that arrived. The sequence we observe must
/// be strictly monotonic, and 15 (the last frame) must appear at
/// the tail. Doesn't depend on the exact number of survivors, only
/// on the ordering invariant + last-frame survivability.
#[test]
fn runner_overflow_drop_oldest_preserves_order_and_keeps_latest_frame() {
    let opened = OpenedCamera::from_device(Box::new(make_sequenced_frame_source(16)));
    let cfg = RunnerConfig {
        frames_capacity: 1,
        overflow: Overflow::DropOldest,
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();

    // Let the runner produce ALL 16 frames. With a 10ms poll_interval
    // default + 5ms relay drain tick, 1s is comfortably enough for
    // the source to drain (16 frames × ~10ms each = ~160ms).
    std::thread::sleep(Duration::from_millis(1000));

    let rx = runner.frames().expect("stream runner has frames channel");
    let mut received = Vec::new();
    while let Ok(buf) = rx.recv_timeout(Duration::from_millis(200)) {
        received.push(buf.buffer()[0]);
    }

    assert!(
        !received.is_empty(),
        "DropOldest must surface at least one frame"
    );
    assert!(
        received.windows(2).all(|w| w[0] < w[1]),
        "DropOldest must preserve order; got {received:?}"
    );
    assert_eq!(
        received.last().copied(),
        Some(15),
        "DropOldest must keep the most recent frame at the tail; got {received:?}"
    );
}

/// `frames_capacity = 0` end-to-end: setting the capacity to 0 in
/// [`RunnerConfig`] must select the unbounded `std::sync::mpsc::channel`
/// path inside `make_channel`, restoring the 0.13-era no-drop behavior
/// regardless of the [`Overflow`] policy. A regression that mis-routes
/// 0 to a `sync_channel(0)` (rendezvous) would deadlock the worker on
/// the first send; one that mis-routes it to `sync_channel(1)` plus
/// `DropNewest` would silently lose frames under burst.
///
/// We push 32 sequenced frames, sleep long enough for the worker to
/// drain the source, then collect everything — every frame must arrive
/// in order with no drops.
#[test]
fn runner_unbounded_capacity_delivers_every_frame_in_order() {
    let opened = OpenedCamera::from_device(Box::new(make_sequenced_frame_source(32)));
    let cfg = RunnerConfig {
        frames_capacity: 0,
        // Policy must be irrelevant when capacity is 0; pick a non-default
        // to ensure the unbounded path doesn't accidentally consult it.
        overflow: Overflow::Block,
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();

    // 32 frames × ~10ms poll_interval = ~320ms; 1s is comfortable.
    std::thread::sleep(Duration::from_millis(1000));

    let rx = runner.frames().expect("stream runner has frames channel");
    let mut received = Vec::new();
    while let Ok(buf) = rx.recv_timeout(Duration::from_millis(200)) {
        received.push(buf.buffer()[0]);
    }

    let expected: Vec<u8> = (0..32).collect();
    assert_eq!(
        received, expected,
        "frames_capacity=0 must deliver every frame in order without drops; got {received:?}"
    );
}

// ───────────── stream worker survives transient frame() errors ────────
//
// `src/runner.rs:309-311` puts the worker thread into a sleep+retry
// loop whenever the underlying `FrameSource::frame()` returns an
// error, on the assumption that errors are transient (V4L2 EAGAIN,
// MSMF sample-not-ready, etc.). If a regression turned that into a
// `break` or `panic!`, the worker would silently die after the first
// transient hiccup and the consumer would never see a recovery, with
// no test catching it.
//
// This test pins resilience via a shared frame queue: produce N frames,
// drain them (queue empties → frame() errors), sleep long enough for
// the worker to walk the error path many times, then push fresh frames
// and verify they arrive.

/// `FrameSource` whose queue is held in an `Arc<Mutex<VecDeque<Buffer>>>`
/// so the test thread can push frames after the worker has started
/// erroring on an empty queue.
struct SharedQueueFrameSource {
    info: CameraInfo,
    format: CameraFormat,
    queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Buffer>>>,
}

impl CameraDevice for SharedQueueFrameSource {
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
        _v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl FrameSource for SharedQueueFrameSource {
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        true
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.queue
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(NokhwaError::TimeoutError(Duration::ZERO))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        match self.queue.lock().unwrap().pop_front() {
            Some(buf) => Ok(Cow::Owned(buf.buffer().to_vec())),
            None => Err(NokhwaError::TimeoutError(Duration::ZERO)),
        }
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

nokhwa_backend!(SharedQueueFrameSource: FrameSource);

#[test]
fn runner_stream_worker_survives_transient_frame_errors() {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    let queue = Arc::new(Mutex::new(VecDeque::<Buffer>::new()));

    // Seed with 3 sequence-tagged frames, numbered 0..3.
    {
        let mut q = queue.lock().unwrap();
        for n in 0..3u8 {
            let mut buf = mock_frame(4, 4, FrameFormat::GRAY).buffer().to_vec();
            buf[0] = n;
            q.push_back(Buffer::from_vec(
                nokhwa_core::types::Resolution::new(4, 4),
                buf,
                FrameFormat::GRAY,
            ));
        }
    }

    let src = SharedQueueFrameSource {
        info: CameraInfo::new(
            "shared".to_string(),
            "shared".to_string(),
            "shared".to_string(),
            false,
            nokhwa_core::types::CameraIndex::Index(0),
        ),
        format: CameraFormat::new(
            nokhwa_core::types::Resolution::new(4, 4),
            FrameFormat::GRAY,
            30,
        ),
        queue: Arc::clone(&queue),
    };

    let opened = OpenedCamera::from_device(Box::new(src));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    let rx = runner.frames().expect("stream runner has frames channel");

    // Drain the initial 3 frames.
    let mut received = Vec::new();
    for _ in 0..3 {
        let buf = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("initial frame timed out");
        received.push(buf.buffer()[0]);
    }
    assert_eq!(received, vec![0, 1, 2]);

    // Source is now empty. The worker keeps polling, hitting the
    // sleep+retry path on each empty-queue frame() error. Sleep long
    // enough to ensure the worker has cycled through that path many
    // times (10ms poll_interval × 30 = 300ms minimum).
    std::thread::sleep(Duration::from_millis(300));

    // Push fresh frames; if the worker died, these never arrive.
    {
        let mut q = queue.lock().unwrap();
        for n in 100u8..103 {
            let mut buf = mock_frame(4, 4, FrameFormat::GRAY).buffer().to_vec();
            buf[0] = n;
            q.push_back(Buffer::from_vec(
                nokhwa_core::types::Resolution::new(4, 4),
                buf,
                FrameFormat::GRAY,
            ));
        }
    }

    let mut after = Vec::new();
    for _ in 0..3 {
        let buf = rx
            .recv_timeout(Duration::from_millis(1000))
            .expect("worker died after transient frame() error");
        after.push(buf.buffer()[0]);
    }
    assert_eq!(
        after,
        vec![100, 101, 102],
        "worker must continue delivering after a stretch of frame() errors; got {after:?}"
    );
}

// ──────────────── set_control E2E forwarding (all 3 workers) ──────────
//
// `CameraRunner::set_control` is documented as queuing
// `Command::SetControl(id, value)` onto the worker's command channel,
// which the worker then forwards to the backend's
// `CameraDevice::set_control`. The path
//   user → cmd channel → worker match arm → backend.set_control
// has no E2E pin. A regression that:
//   (a) routes `Command::SetControl` into the `Trigger` / `Empty` no-op
//       arms by accident (e.g. during a refactor that re-orders the
//       match), or
//   (b) drops the worker arm entirely for one of the three variants
// would silently turn `runner.set_control(...)` into a no-op without
// any test failing.
//
// Strategy: use an `Arc<Mutex<Vec<(KnownCameraControl, ControlValueSetter)>>>`
// probe shared with each variant's mock; assert the recorded calls
// match the user's `runner.set_control(...)` invocations.

type SetControlLog =
    std::sync::Arc<std::sync::Mutex<Vec<(KnownCameraControl, ControlValueSetter)>>>;

fn poll_log_until<F: Fn(&[(KnownCameraControl, ControlValueSetter)]) -> bool>(
    log: &SetControlLog,
    pred: F,
    timeout: Duration,
) -> Vec<(KnownCameraControl, ControlValueSetter)> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let snapshot = log.lock().unwrap().clone();
        if pred(&snapshot) {
            return snapshot;
        }
        if std::time::Instant::now() >= deadline {
            return snapshot;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Stream-only probe.
struct ControlProbeStream {
    info: CameraInfo,
    format: CameraFormat,
    log: SetControlLog,
}
impl CameraDevice for ControlProbeStream {
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
        id: KnownCameraControl,
        v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.log.lock().unwrap().push((id, v));
        Ok(())
    }
}
impl FrameSource for ControlProbeStream {
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        true
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        Err(NokhwaError::TimeoutError(Duration::ZERO))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Err(NokhwaError::TimeoutError(Duration::ZERO))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}
nokhwa_backend!(ControlProbeStream: FrameSource);

/// Shutter-only probe.
struct ControlProbeShutter {
    info: CameraInfo,
    log: SetControlLog,
}
impl CameraDevice for ControlProbeShutter {
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
        id: KnownCameraControl,
        v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.log.lock().unwrap().push((id, v));
        Ok(())
    }
}
impl ShutterCapture for ControlProbeShutter {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn take_picture(&mut self, _t: Duration) -> Result<Buffer, NokhwaError> {
        Ok(mock_frame(4, 4, FrameFormat::MJPEG))
    }
}
nokhwa_backend!(ControlProbeShutter: ShutterCapture);

/// Hybrid probe — frames + shutter, no events.
struct ControlProbeHybrid {
    info: CameraInfo,
    format: CameraFormat,
    log: SetControlLog,
}
impl CameraDevice for ControlProbeHybrid {
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
        id: KnownCameraControl,
        v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.log.lock().unwrap().push((id, v));
        Ok(())
    }
}
impl FrameSource for ControlProbeHybrid {
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        true
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        Err(NokhwaError::TimeoutError(Duration::ZERO))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Err(NokhwaError::TimeoutError(Duration::ZERO))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}
impl ShutterCapture for ControlProbeHybrid {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn take_picture(&mut self, _t: Duration) -> Result<Buffer, NokhwaError> {
        Ok(mock_frame(4, 4, FrameFormat::MJPEG))
    }
}
nokhwa_backend!(ControlProbeHybrid: FrameSource, ShutterCapture);

fn make_probe_info() -> CameraInfo {
    CameraInfo::new(
        "probe".to_string(),
        "probe".to_string(),
        "probe".to_string(),
        false,
        nokhwa_core::types::CameraIndex::Index(0),
    )
}

fn make_probe_format() -> CameraFormat {
    CameraFormat::new(
        nokhwa_core::types::Resolution::new(4, 4),
        FrameFormat::GRAY,
        30,
    )
}

#[test]
fn runner_set_control_forwards_to_stream_worker() {
    let log: SetControlLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = ControlProbeStream {
        info: make_probe_info(),
        format: make_probe_format(),
        log: SetControlLog::clone(&log),
    };
    let opened = OpenedCamera::from_device(Box::new(probe));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner
        .set_control(
            KnownCameraControl::Brightness,
            ControlValueSetter::Float(0.5),
        )
        .unwrap();
    runner
        .set_control(KnownCameraControl::Contrast, ControlValueSetter::Integer(7))
        .unwrap();
    let snapshot = poll_log_until(&log, |s| s.len() >= 2, Duration::from_millis(500));
    assert_eq!(
        snapshot,
        vec![
            (
                KnownCameraControl::Brightness,
                ControlValueSetter::Float(0.5)
            ),
            (KnownCameraControl::Contrast, ControlValueSetter::Integer(7)),
        ],
        "stream worker must forward Command::SetControl to backend.set_control"
    );
}

#[test]
fn runner_set_control_forwards_to_shutter_worker() {
    let log: SetControlLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = ControlProbeShutter {
        info: make_probe_info(),
        log: SetControlLog::clone(&log),
    };
    let opened = OpenedCamera::from_device(Box::new(probe));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner
        .set_control(
            KnownCameraControl::Saturation,
            ControlValueSetter::Boolean(true),
        )
        .unwrap();
    let snapshot = poll_log_until(&log, |s| !s.is_empty(), Duration::from_millis(500));
    assert_eq!(
        snapshot,
        vec![(
            KnownCameraControl::Saturation,
            ControlValueSetter::Boolean(true)
        )],
        "shutter worker must forward Command::SetControl to backend.set_control"
    );
}

#[test]
fn runner_set_control_forwards_to_hybrid_worker() {
    let log: SetControlLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = ControlProbeHybrid {
        info: make_probe_info(),
        format: make_probe_format(),
        log: SetControlLog::clone(&log),
    };
    let opened = OpenedCamera::from_device(Box::new(probe));
    let runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();
    runner
        .set_control(KnownCameraControl::Hue, ControlValueSetter::Integer(42))
        .unwrap();
    runner
        .set_control(KnownCameraControl::Gamma, ControlValueSetter::Float(2.2))
        .unwrap();
    let snapshot = poll_log_until(&log, |s| s.len() >= 2, Duration::from_millis(500));
    assert_eq!(
        snapshot,
        vec![
            (KnownCameraControl::Hue, ControlValueSetter::Integer(42)),
            (KnownCameraControl::Gamma, ControlValueSetter::Float(2.2)),
        ],
        "hybrid worker must forward Command::SetControl to backend.set_control"
    );
}

// ───────── hybrid: dropped pictures receiver keeps frame stream alive ─
//
// `src/runner.rs:432-437` documents an explicit asymmetry: a dropped
// *frames* receiver exits the hybrid worker (line 449-451), but a
// dropped *pictures* receiver is silently swallowed
// (`let _ = pic_tx.send(pic)`), keeping the frame stream alive. This
// "one-shot photo while streaming" use case has no test pin. A
// regression that mirrored the frames path here (`if pic_tx.send(...)
// .is_err() { break }`) would silently kill the frame stream whenever
// a caller triggers a shot and then drops the picture receiver.

#[test]
fn runner_hybrid_dropped_pictures_receiver_keeps_frame_stream_alive() {
    // Use a hybrid that always has frames and at least one picture
    // queued so trigger() actually causes a `pic_tx.send(pic)` call.
    let mut h = MockHybrid::new(0, vec![mock_frame(4, 4, FrameFormat::MJPEG)]);
    for _ in 0..32 {
        h.push_frame(mock_frame(8, 8, FrameFormat::YUYV));
    }
    let opened = OpenedCamera::from_device(Box::new(HybridWithEvents::new(
        h,
        Box::new(MpscEventPoll::new(channel().1)),
    )));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();

    // Drain one frame to confirm the stream is producing.
    let _ = runner
        .frames()
        .unwrap()
        .recv_timeout(Duration::from_millis(500))
        .expect("frame stream not producing before pictures-drop");

    // Take the pictures receiver and drop it immediately — the worker
    // now has a closed pictures channel.
    let pic_rx = runner.take_pictures().expect("hybrid has pictures channel");
    drop(pic_rx);

    // Trigger a shot. The worker must NOT exit just because pic_tx.send
    // fails — that's the documented policy.
    runner.trigger().unwrap();

    // Frames must continue arriving after the trigger that landed on a
    // closed pictures channel. Pull at least 3 frames to guard against
    // the worker being mid-cycle when we triggered.
    for i in 0..3 {
        runner
            .frames()
            .unwrap()
            .recv_timeout(Duration::from_millis(1000))
            .unwrap_or_else(|_| {
                panic!(
                    "hybrid worker exited after dropped pictures receiver \
                     (frame {i} timed out); src/runner.rs:432-437 policy regressed"
                )
            });
    }
}

// ──────── event worker: receiver-drop shutdown + event_tick forwarding ─
//
// `src/runner.rs:403-412` defines a separate event-poll thread that
// `poll.next_timeout(event_tick)`s in a loop and forwards each event
// through `ev_tx`. Two contracts are silently degradable:
//
//   1. **Receiver-drop self-shutdown** (line 408-410). If the user
//      drops the events receiver, the next `ev_tx.send(event)` returns
//      `SendError`; the worker breaks out of the loop. A regression
//      that turned that into `let _ = ev_tx.send(event)` would leak
//      the event thread per-runner — invisible until thread-count
//      monitoring or a long-running test caught it.
//
//   2. **`event_tick` forwarding** (line 402, 407). The user's
//      `RunnerConfig::event_tick` is captured into the closure and
//      passed verbatim to `poll.next_timeout(event_tick)`. A regression
//      that hard-coded `Duration::from_millis(50)` (the default), or
//      passed `Duration::ZERO`, or accidentally swapped to `poll_interval`,
//      would silently change the event-poll cadence. Existing test
//      `runner_config_default_pins_field_values` only checks the
//      default *value*, not that it's actually used.

/// `EventPoll` that records the `Duration` it was last called with,
/// emits events from a shared queue, and respects a "stop" flag the
/// test thread can flip to make `next_timeout` return `None` forever.
struct RecordingEventPoll {
    last_timeout: std::sync::Arc<std::sync::Mutex<Option<Duration>>>,
    queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<CameraEvent>>>,
}

impl EventPoll for RecordingEventPoll {
    fn try_next(&mut self) -> Option<CameraEvent> {
        self.queue.lock().unwrap().pop_front()
    }
    fn next_timeout(&mut self, d: Duration) -> Option<CameraEvent> {
        *self.last_timeout.lock().unwrap() = Some(d);
        let popped = self.queue.lock().unwrap().pop_front();
        if popped.is_none() {
            // Sleep briefly so the worker doesn't busy-loop hammering
            // the lock when the queue is empty (matches the contract
            // of a real `EventPoll` blocking up to `d`).
            std::thread::sleep(Duration::from_millis(10));
        }
        popped
    }
}

#[test]
fn runner_event_worker_forwards_event_tick_to_poll() {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    let last_timeout: Arc<Mutex<Option<Duration>>> = Arc::new(Mutex::new(None));
    let queue: Arc<Mutex<VecDeque<CameraEvent>>> = Arc::new(Mutex::new(VecDeque::new()));
    let poll = RecordingEventPoll {
        last_timeout: Arc::clone(&last_timeout),
        queue: Arc::clone(&queue),
    };

    // Hybrid with at least one frame so the frames worker has work
    // (otherwise it would spin in the error-sleep path; harmless but
    // wastes cycles).
    let mut h = MockHybrid::new(0, vec![mock_frame(4, 4, FrameFormat::MJPEG)]);
    h.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
    let dev = HybridWithEvents::new(h, Box::new(poll));
    let opened = OpenedCamera::from_device(Box::new(dev));

    let custom_tick = Duration::from_millis(173);
    let cfg = RunnerConfig {
        event_tick: custom_tick,
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();

    // Wait until the event worker has called `next_timeout` at least
    // once; 100ms is comfortably more than the 10ms sleep inside the
    // poll's empty-queue path.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        if last_timeout.lock().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("event worker never called RecordingEventPoll::next_timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        last_timeout.lock().unwrap().unwrap(),
        custom_tick,
        "RunnerConfig::event_tick must be forwarded verbatim to EventPoll::next_timeout"
    );

    // Clean shutdown to avoid leaking the worker into the next test.
    runner.stop().unwrap();
}

#[test]
fn runner_event_worker_exits_when_events_receiver_dropped() {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    let last_timeout: Arc<Mutex<Option<Duration>>> = Arc::new(Mutex::new(None));
    let queue: Arc<Mutex<VecDeque<CameraEvent>>> = Arc::new(Mutex::new(VecDeque::new()));
    let poll = RecordingEventPoll {
        last_timeout: Arc::clone(&last_timeout),
        queue: Arc::clone(&queue),
    };

    let mut h = MockHybrid::new(0, vec![mock_frame(4, 4, FrameFormat::MJPEG)]);
    h.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
    let dev = HybridWithEvents::new(h, Box::new(poll));
    let opened = OpenedCamera::from_device(Box::new(dev));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();

    // Take + drop the events receiver so the next `ev_tx.send` errors.
    let ev_rx = runner.take_events().expect("hybrid has events channel");
    drop(ev_rx);

    // Push enough events that the worker is guaranteed to attempt at
    // least one `ev_tx.send` after the receiver is gone.
    {
        let mut q = queue.lock().unwrap();
        for _ in 0..8 {
            q.push_back(CameraEvent::Disconnected);
        }
    }

    // The worker must observe the SendError and exit. We can't directly
    // observe its `JoinHandle`, but we can verify the runner stops
    // cleanly within a bounded time — `stop()` joins the main worker,
    // which in turn joins the event worker via the `(ev_cmd_tx, handle)`
    // pair. If the event worker leaked, this would hang forever.
    let stopper = std::thread::spawn(move || runner.stop());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !stopper.is_finished() {
        if std::time::Instant::now() >= deadline {
            panic!(
                "runner.stop() hung; event worker likely did not exit on \
                 dropped events receiver (src/runner.rs:408-410 regressed)"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stopper.join().unwrap().unwrap();
}

// ───── spawn_hybrid: take_events() Err is swallowed (events=None) ─────
//
// `src/runner.rs:368-378` discriminates three cases when constructing
// the events channel:
//
//   - `Some(Ok(poll))` → spin up the event worker thread.
//   - `Some(Err(e))`   → log + treat the same as `None` (no event
//                        worker, no events receiver).
//   - `None`           → backend has no `EventSource` capability.
//
// The middle arm is exercised whenever a hybrid backend advertises
// `EventSource` capability but the runtime acquisition fails (e.g. the
// inotify fd / RegisterDeviceNotificationW window couldn't be created).
// A regression that propagated that error out of `spawn_hybrid` would
// turn a transient OS-resource failure into "the entire camera runner
// refuses to spawn" — frames + pictures would never start.

/// Hybrid backend whose `take_events()` always returns `Err`.
struct HybridEventErr {
    inner: MockHybrid,
}

impl CameraDevice for HybridEventErr {
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
impl FrameSource for HybridEventErr {
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
impl ShutterCapture for HybridEventErr {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.inner.trigger()
    }
    fn take_picture(&mut self, t: Duration) -> Result<Buffer, NokhwaError> {
        self.inner.take_picture(t)
    }
}
impl EventSource for HybridEventErr {
    fn take_events(&mut self) -> Result<Box<dyn EventPoll + Send>, NokhwaError> {
        Err(NokhwaError::UnsupportedOperationError(ApiBackend::Browser))
    }
}

nokhwa_backend!(HybridEventErr: FrameSource, ShutterCapture, EventSource);

#[test]
fn runner_spawn_hybrid_swallows_take_events_error() {
    let mut h = MockHybrid::new(0, vec![mock_frame(4, 4, FrameFormat::MJPEG)]);
    for _ in 0..8 {
        h.push_frame(mock_frame(4, 4, FrameFormat::YUYV));
    }
    let dev = HybridEventErr { inner: h };
    let opened = OpenedCamera::from_device(Box::new(dev));

    // spawn must NOT propagate the take_events error out — the runner
    // should come up healthy with events=None.
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default())
        .expect("spawn must swallow take_events() Err and return Ok");

    assert!(
        runner.events().is_none(),
        "events channel must be absent when backend's take_events() errored"
    );
    assert!(
        runner.take_events().is_none(),
        "take_events() must be None when backend's take_events() errored"
    );

    // Frames must still flow — the take_events failure must not have
    // poisoned the frame worker.
    runner
        .frames()
        .expect("hybrid runner must still expose frames channel")
        .recv_timeout(Duration::from_millis(500))
        .expect("frame must arrive after take_events Err was swallowed");
}

// ───── spawn_stream / spawn_hybrid: open() error propagates ──────────
//
// `src/runner.rs:288` (`spawn_stream`) and `src/runner.rs:367`
// (`spawn_hybrid`) both start with `cam.open()?` before doing any
// channel / thread setup. The contract is: if the backend's `open()`
// returns `Err`, `CameraRunner::spawn` must propagate that error to the
// caller — no thread is spawned, no `Ok(runner)` is returned.
//
// A regression that silenced this (e.g. `let _ = cam.open();` or
// reordering thread spawn before `open()`) would produce a "healthy"
// runner whose frames channel never delivers anything, since the
// worker thread loops calling `cam.frame()` on an unopened device that
// errors every iteration. The user would see no error at spawn time
// and only notice silence on the receiver — a particularly nasty
// debugging experience.
//
// `spawn_shutter` does NOT call `open()` (shutter backends are
// triggered, not streamed), so the open-error contract is
// stream/hybrid-only.

/// `FrameSource` that always returns `Err` from `open()`. Other methods
/// would never be called in the open-failure path; we error them too so
/// any regression that swallowed the open error and proceeded into the
/// worker loop would still be observable as a stuck runner.
struct FailingOpenFrame {
    info: CameraInfo,
}

impl CameraDevice for FailingOpenFrame {
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
        _v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}
impl FrameSource for FailingOpenFrame {
    fn negotiated_format(&self) -> CameraFormat {
        CameraFormat::default()
    }
    fn set_format(&mut self, _f: CameraFormat) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        Ok(vec![])
    }
    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        Ok(vec![])
    }
    fn open(&mut self) -> Result<(), NokhwaError> {
        Err(NokhwaError::open_stream("simulated open() failure"))
    }
    fn is_open(&self) -> bool {
        false
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        Err(NokhwaError::read_frame("not open"))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Err(NokhwaError::read_frame("not open"))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

nokhwa_backend!(FailingOpenFrame: FrameSource);

#[test]
fn runner_spawn_stream_propagates_open_error() {
    let dev = FailingOpenFrame {
        info: CameraInfo::new(
            "FailingOpenFrame".to_string(),
            "test".to_string(),
            "test".to_string(),
            false,
            nokhwa_core::types::CameraIndex::Index(0),
        ),
    };
    let opened = OpenedCamera::from_device(Box::new(dev));
    let result = CameraRunner::spawn(opened, RunnerConfig::default());

    let err = result.expect_err(
        "spawn_stream must propagate open() error rather than return Ok with a stuck worker",
    );
    // Pin the full Display string. `cam.open()?` (`src/runner.rs:288`)
    // surfaces the inner `NokhwaError::open_stream(...)` verbatim, so
    // a regression that wraps the error in a different variant
    // (e.g. `GeneralError`, `OpenDeviceError`) — or strips the
    // `"Could not open device stream: "` prefix from the
    // `OpenStreamError` Display impl (`nokhwa-core/src/error.rs:48`)
    // — would still satisfy a `contains("simulated open() failure")`
    // check while breaking every downstream log scraper.
    assert_eq!(
        format!("{err}"),
        "Could not open device stream: simulated open() failure"
    );
}

/// Hybrid that always returns `Err` from `FrameSource::open()`. Built
/// over a `MockHybrid` so the rest of the trait surface is realistic.
struct FailingOpenHybrid {
    inner: MockHybrid,
}

impl CameraDevice for FailingOpenHybrid {
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
impl FrameSource for FailingOpenHybrid {
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
        Err(NokhwaError::open_stream("simulated hybrid open() failure"))
    }
    fn is_open(&self) -> bool {
        false
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        Err(NokhwaError::read_frame("not open"))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Err(NokhwaError::read_frame("not open"))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}
impl ShutterCapture for FailingOpenHybrid {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.inner.trigger()
    }
    fn take_picture(&mut self, t: Duration) -> Result<Buffer, NokhwaError> {
        self.inner.take_picture(t)
    }
}

nokhwa_backend!(FailingOpenHybrid: FrameSource, ShutterCapture);

#[test]
fn runner_spawn_hybrid_propagates_open_error() {
    let dev = FailingOpenHybrid {
        inner: MockHybrid::new(0, vec![mock_frame(4, 4, FrameFormat::MJPEG)]),
    };
    let opened = OpenedCamera::from_device(Box::new(dev));
    let result = CameraRunner::spawn(opened, RunnerConfig::default());

    let err = result.expect_err(
        "spawn_hybrid must propagate open() error rather than return Ok with a stuck worker",
    );
    // Mirror the stream-spawn pin above: `spawn_hybrid` (`src/runner.rs:366`)
    // also calls `cam.open()?` and surfaces the inner
    // `NokhwaError::open_stream(...)` verbatim. Pin the full Display string
    // for the hybrid path so a regression that wraps / rewords the error
    // surfaces here and not in user-facing logs.
    assert_eq!(
        format!("{err}"),
        "Could not open device stream: simulated hybrid open() failure"
    );
}

// ──────── spawn_stream worker exits on dropped frames receiver ────────
//
// `src/runner.rs:305-307` documents that the stream worker treats a
// dropped frames receiver as a shutdown signal:
//
//     if frame_tx.send(buf).is_err() {
//         break;
//     }
//
// The receiver-drop policy is asymmetric to the dropped-pictures arm in
// `spawn_hybrid` (which silently swallows the send-error and keeps
// streaming): for the stream-only worker, frames are the *only* output
// channel, so a dropped consumer means there is no work left to do.
// A regression that swapped to `let _ = frame_tx.send(buf)` would leak
// the worker thread per-runner — invisible until thread-count monitoring
// or a long-running test caught it.
//
// The hybrid path's parallel `frame_tx.send(...).is_err() { break }` at
// `src/runner.rs:449-451` is similarly un-pinned for the *frames*-drop
// case (the existing `runner_hybrid_dropped_pictures_receiver_keeps_frame_stream_alive`
// only covers the picture-drop asymmetry). This test deliberately scopes
// to the stream worker so the observable channel is unambiguous.
//
// Observable contract: after dropping the frames receiver, the worker's
// next `frame_tx.send(buf)` returns `SendError`, the worker breaks the
// loop, and `cmd_rx` (which the worker owns) drops. Subsequent calls
// to `set_control` / `trigger` on the runner — both forward to the
// command channel via `cmd.send(...)` — must return `Err("runner thread
// gone: ...")` because the worker's `cmd_rx` is closed.

/// `FrameSource` that produces an unbounded sequence of fresh frames so
/// the worker is always on the `frame_tx.send` path (never on the
/// sleep+retry frame()-error path). Required to make the
/// `is_err() { break }` branch deterministic — a finite seed could let
/// the worker exit via empty-queue errors before observing the dropped
/// receiver.
struct EndlessFrameSource {
    info: CameraInfo,
    format: CameraFormat,
}

impl CameraDevice for EndlessFrameSource {
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
        _v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl FrameSource for EndlessFrameSource {
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        true
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        Ok(mock_frame(4, 4, self.format.format()))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Ok(Cow::Owned(
            mock_frame(4, 4, self.format.format()).buffer().to_vec(),
        ))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

nokhwa_backend!(EndlessFrameSource: FrameSource);

#[test]
fn runner_spawn_stream_worker_exits_on_dropped_frames_receiver() {
    use nokhwa_core::types::CameraIndex;
    use std::time::Instant;

    let src = EndlessFrameSource {
        info: CameraInfo::new(
            "endless".to_string(),
            "endless".to_string(),
            "endless".to_string(),
            false,
            CameraIndex::Index(0),
        ),
        format: CameraFormat::new(
            nokhwa_core::types::Resolution::new(4, 4),
            FrameFormat::YUYV,
            30,
        ),
    };
    let opened = OpenedCamera::from_device(Box::new(src));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();

    // Drain one frame to confirm the worker is on the send path before
    // we drop the receiver. Without this confirmation, a regression that
    // never reaches `frame_tx.send` (e.g. a worker that exited at startup)
    // could pass spuriously.
    let rx = runner
        .take_frames()
        .expect("stream runner has frames channel");
    rx.recv_timeout(Duration::from_millis(500))
        .expect("frame stream not producing before frames-drop");

    // Drop the frames receiver. The worker's next `frame_tx.send(buf)`
    // call must return SendError, after which it breaks the loop and
    // drops `cmd_rx`.
    drop(rx);

    // Observable: once the worker has exited, `cmd.send(...)` (used by
    // `set_control` and `trigger`) returns `SendError` because the
    // worker dropped `cmd_rx`. Poll until that flips, with a 3-second
    // deadline. If the test hangs, the worker is leaking — exactly the
    // regression this test is here to catch.
    //
    // `set_control` (`src/runner.rs:517-520`) maps the `SendError` through
    // `NokhwaError::general(format!("runner thread gone: {e}"))`. Pin
    // both the variant and the `"runner thread gone: "` prefix so a
    // future refactor that swapped the wrapping (e.g. to
    // `OpenStreamError`, `StreamShutdownError`, or stripped the prefix)
    // would fail this test instead of being absorbed by `is_err()`.
    let deadline = Instant::now() + Duration::from_secs(3);
    let err = loop {
        if Instant::now() >= deadline {
            panic!(
                "stream worker did not exit after dropping frames receiver — \
                 src/runner.rs:305-307 receiver-drop policy regressed \
                 (set_control still routes to a live worker after 3s)"
            );
        }
        match runner.set_control(
            KnownCameraControl::Brightness,
            ControlValueSetter::Integer(0),
        ) {
            Ok(()) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => break e,
        }
    };
    match err {
        NokhwaError::GeneralError { message, backend } => {
            assert!(
                message.starts_with("runner thread gone: "),
                "expected message starting with 'runner thread gone: ', got {message:?}"
            );
            assert!(
                backend.is_none(),
                "expected no backend tag, got {backend:?}"
            );
        },
        other => panic!("expected GeneralError, got {other:?}"),
    }
}

// ──────── spawn_hybrid worker exits on dropped frames receiver ────────
//
// Symmetric to `runner_spawn_stream_worker_exits_on_dropped_frames_receiver`
// but for the hybrid worker at `src/runner.rs:449-451`:
//
//     match cam.frame() {
//         Ok(buf) => {
//             // Exit if the consumer dropped the frames receiver.
//             if frame_tx.send(buf).is_err() {
//                 break;
//             }
//         }
//         ...
//     }
//
// `runner_hybrid_dropped_pictures_receiver_keeps_frame_stream_alive`
// covers the *picture*-drop asymmetry (worker keeps streaming on a
// dropped pictures receiver). The frames-drop arm — where the hybrid
// worker actually does shut down — was un-pinned. A regression that
// mirrored the picture-drop policy on this arm (`let _ = frame_tx.send(buf)`)
// would leak the hybrid worker per-runner, with the same invisible
// failure mode as the stream worker.
//
// The hybrid worker also signals the event thread to stop via
// `ev_cmd_tx.send(())` after the main loop breaks (`src/runner.rs:459-466`),
// so this test indirectly exercises that "frames-drop tears down both
// threads cleanly" path too.

/// Hybrid backend that always returns `Ok` from `frame()` (so the worker
/// is always on the send path) and `Ok` from `trigger`/`take_picture`
/// (the test never triggers, but the trait must be implementable).
/// Mirrors `EndlessFrameSource` for the hybrid case.
struct EndlessHybrid {
    info: CameraInfo,
    format: CameraFormat,
}

impl CameraDevice for EndlessHybrid {
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
        _v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl FrameSource for EndlessHybrid {
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        true
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        Ok(mock_frame(4, 4, self.format.format()))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Ok(Cow::Owned(
            mock_frame(4, 4, self.format.format()).buffer().to_vec(),
        ))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl ShutterCapture for EndlessHybrid {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn take_picture(&mut self, _t: Duration) -> Result<Buffer, NokhwaError> {
        Ok(mock_frame(4, 4, self.format.format()))
    }
}

nokhwa_backend!(EndlessHybrid: FrameSource, ShutterCapture);

#[test]
fn runner_spawn_hybrid_worker_exits_on_dropped_frames_receiver() {
    use nokhwa_core::types::CameraIndex;
    use std::time::Instant;

    let dev = EndlessHybrid {
        info: CameraInfo::new(
            "endless-hybrid".to_string(),
            "endless-hybrid".to_string(),
            "endless-hybrid".to_string(),
            false,
            CameraIndex::Index(0),
        ),
        format: CameraFormat::new(
            nokhwa_core::types::Resolution::new(4, 4),
            FrameFormat::YUYV,
            30,
        ),
    };
    let opened = OpenedCamera::from_device(Box::new(dev));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();

    // Drain one frame to confirm the worker is on the send path.
    let rx = runner
        .take_frames()
        .expect("hybrid runner has frames channel");
    rx.recv_timeout(Duration::from_millis(500))
        .expect("hybrid frame stream not producing before frames-drop");

    // Drop the frames receiver. The next iteration of the worker loop
    // hits `frame_tx.send(buf).is_err()` and breaks.
    drop(rx);

    // Once the worker exits, `cmd_rx` drops; subsequent `cmd.send(...)`
    // returns `SendError`, flipping `set_control` to `Err`. 3-second
    // deadline catches a leaked hybrid worker. See the stream sibling
    // for the rationale on pinning the `"runner thread gone: "` prefix.
    let deadline = Instant::now() + Duration::from_secs(3);
    let err = loop {
        if Instant::now() >= deadline {
            panic!(
                "hybrid worker did not exit after dropping frames receiver — \
                 src/runner.rs:449-451 receiver-drop policy regressed \
                 (set_control still routes to a live worker after 3s)"
            );
        }
        match runner.set_control(
            KnownCameraControl::Brightness,
            ControlValueSetter::Integer(0),
        ) {
            Ok(()) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => break e,
        }
    };
    match err {
        NokhwaError::GeneralError { message, backend } => {
            assert!(
                message.starts_with("runner thread gone: "),
                "expected message starting with 'runner thread gone: ', got {message:?}"
            );
            assert!(
                backend.is_none(),
                "expected no backend tag, got {backend:?}"
            );
        },
        other => panic!("expected GeneralError, got {other:?}"),
    }
}

// ─────── spawn_shutter worker exits on dropped pictures receiver ──────
//
// Third member of the receiver-drop family. `src/runner.rs:340-342`
// inside `spawn_shutter`:
//
//     if cam.trigger().is_ok() {
//         if let Ok(pic) = cam.take_picture(shutter_timeout) {
//             if pic_tx.send(pic).is_err() {
//                 break;
//             }
//         }
//     }
//
// The shutter worker is event-driven: it sleeps inside
// `cmd_rx.recv_timeout(poll_interval)` and only attempts `pic_tx.send`
// after a successful `Command::Trigger`. So the test must (a) drop the
// pictures receiver, (b) issue a trigger, and (c) wait for the worker
// to exit via the `is_err() { break }` arm.
//
// A regression that swapped to `let _ = pic_tx.send(pic)` would leak
// the shutter worker, same invisible failure mode as the stream/hybrid
// frames-drop counterparts (PRs #304, #305).

/// `ShutterCapture` that always succeeds. Required so `trigger()` →
/// `take_picture` reliably reaches the `pic_tx.send` call after we drop
/// the receiver — a finite `MockShutter` runs out and short-circuits via
/// the `take_picture` `TimeoutError` arm before reaching `pic_tx.send`.
struct EndlessShutter {
    info: CameraInfo,
}

impl CameraDevice for EndlessShutter {
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
        _v: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl ShutterCapture for EndlessShutter {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn take_picture(&mut self, _t: Duration) -> Result<Buffer, NokhwaError> {
        Ok(mock_frame(4, 4, FrameFormat::MJPEG))
    }
}

nokhwa_backend!(EndlessShutter: ShutterCapture);

#[test]
fn runner_spawn_shutter_worker_exits_on_dropped_pictures_receiver() {
    use nokhwa_core::types::CameraIndex;
    use std::time::Instant;

    let dev = EndlessShutter {
        info: CameraInfo::new(
            "endless-shutter".to_string(),
            "endless-shutter".to_string(),
            "endless-shutter".to_string(),
            false,
            CameraIndex::Index(0),
        ),
    };
    let opened = OpenedCamera::from_device(Box::new(dev));
    let mut runner = CameraRunner::spawn(opened, RunnerConfig::default()).unwrap();

    // Trigger once and drain a picture to confirm the worker is live and
    // on the send path before we drop the receiver. Without this, a
    // regression that exited the worker at startup could pass spuriously.
    runner.trigger().unwrap();
    let rx = runner
        .take_pictures()
        .expect("shutter runner has pictures channel");
    rx.recv_timeout(Duration::from_millis(500))
        .expect("shutter pictures not arriving before pictures-drop");

    // Drop the pictures receiver. The next trigger causes the worker to
    // hit `pic_tx.send(pic).is_err()` and break.
    drop(rx);
    runner.trigger().unwrap();

    // Once the worker exits, `cmd_rx` drops; subsequent `cmd.send(...)`
    // returns `SendError`, flipping `set_control` to `Err`. 3-second
    // deadline catches a leaked shutter worker. See the stream sibling
    // for the rationale on pinning the `"runner thread gone: "` prefix.
    let deadline = Instant::now() + Duration::from_secs(3);
    let err = loop {
        if Instant::now() >= deadline {
            panic!(
                "shutter worker did not exit after dropping pictures receiver — \
                 src/runner.rs:340-342 receiver-drop policy regressed \
                 (set_control still routes to a live worker after 3s)"
            );
        }
        match runner.set_control(
            KnownCameraControl::Brightness,
            ControlValueSetter::Integer(0),
        ) {
            Ok(()) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => break e,
        }
    };
    match err {
        NokhwaError::GeneralError { message, backend } => {
            assert!(
                message.starts_with("runner thread gone: "),
                "expected message starting with 'runner thread gone: ', got {message:?}"
            );
            assert!(
                backend.is_none(),
                "expected no backend tag, got {backend:?}"
            );
        },
        other => panic!("expected GeneralError, got {other:?}"),
    }
}

// ────────── shutdown ordering: relay must not deadlock on close ──────
//
// `src/runner.rs:206-220` documents an explicit ordering invariant of
// the `DropOldest` relay loop:
//
//     // The blocking `user_tx.send` here is safe only because
//     // `CameraRunner::shutdown` drops the user-facing `Receiver`
//     // *before* joining the relay — so if no one is draining,
//     // `send` fails with `SendError` and we exit. Future refactors
//     // of `shutdown` must preserve that ordering or this loop can
//     // deadlock.
//
// The current `shutdown()` (`src/runner.rs:532-558`):
//   1. send `Command::Die` to worker
//   2. drop user-facing receivers (frames / pictures / events)
//   3. join the worker handle
//   4. join each relay handle
//
// A regression that swapped (2) and (4) — joining relays before
// dropping the receivers — would let the relay's `user_tx.send(front)`
// in the producer-disconnected drain loop block forever waiting for a
// reader that no longer exists, and `relay.join()` would never return.
// `runner.stop()` (or `Drop`) would hang.
//
// Test: build a `DropOldest` runner with `frames_capacity = 1` driven
// by `EndlessFrameSource` so the worker continually pushes through the
// relay. Don't drain anything — the user-side channel and the relay's
// VecDeque both fill. Then call `runner.stop()` on a side thread with
// a 3-second deadline. With the correct ordering, the relay's drain
// loop's `user_tx.send` returns `SendError` immediately and the join
// completes; with reversed ordering the join hangs. Mirrors the
// stop-with-deadline pattern used by
// `runner_event_worker_exits_when_events_receiver_dropped`.

#[test]
fn runner_shutdown_drops_receivers_before_joining_drop_oldest_relay() {
    use nokhwa_core::types::CameraIndex;
    use std::time::Instant;

    let src = EndlessFrameSource {
        info: CameraInfo::new(
            "endless-relay".to_string(),
            "endless-relay".to_string(),
            "endless-relay".to_string(),
            false,
            CameraIndex::Index(0),
        ),
        format: CameraFormat::new(
            nokhwa_core::types::Resolution::new(4, 4),
            FrameFormat::YUYV,
            30,
        ),
    };
    let opened = OpenedCamera::from_device(Box::new(src));
    let cfg = RunnerConfig {
        // Capacity 1 + DropOldest forces the relay to be the bottleneck:
        // worker keeps pushing, the relay's VecDeque/user_tx pair fills,
        // and the producer-disconnect drain loop is the path that has
        // to exit cleanly. Larger capacities still pass but exercise
        // the contract less aggressively.
        frames_capacity: 1,
        overflow: Overflow::DropOldest,
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();

    // Let the worker run long enough to fill the relay's VecDeque +
    // user_tx (capacity 1 each, plus prod_tx capacity 1 = 3 in-flight).
    // 100 ms at 10 ms `poll_interval` is ~10 iterations of the worker
    // loop — well past saturation.
    std::thread::sleep(Duration::from_millis(100));

    // Drop without ever calling `frames()` / `take_frames()` to drain.
    // If `shutdown()` reverses the documented ordering, the relay's
    // `user_tx.send(front)` in the producer-disconnect drain loop
    // blocks forever and `relay.join()` never returns.
    let stopper = std::thread::spawn(move || runner.stop());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !stopper.is_finished() {
        if Instant::now() >= deadline {
            panic!(
                "runner.stop() hung; DropOldest relay likely deadlocked \
                 in user_tx.send because shutdown() joined the relay \
                 before dropping the user-facing receiver \
                 (src/runner.rs:206-220 ordering invariant regressed)"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stopper.join().unwrap().unwrap();
}

// ─── shutdown ordering: Block worker parked in SyncSender::send ──────
//
// Symmetric companion to
// `runner_shutdown_drops_receivers_before_joining_drop_oldest_relay`.
// `src/runner.rs:534-540` explicitly documents the contract for
// `Overflow::Block`:
//
//     // Drop receivers first so any blocked relay `try_send` wakes up
//     // and the relay threads can exit cleanly. For `Overflow::Block`
//     // the worker may be parked inside `SyncSender::send` with `Die`
//     // sitting unread in the command queue; the receiver-drop below is
//     // what unblocks that send (via `SendError`), after which the
//     // worker loop exits.
//
// Order in `shutdown()` (`src/runner.rs:532-558`):
//   1. send `Command::Die` to worker
//   2. drop user-facing receivers (frames / pictures / events)
//   3. join the worker handle
//   4. join each relay handle
//
// For `Overflow::Block`, `make_channel` returns the user `SyncSender`
// directly to the worker (no relay), so the worker can park inside the
// blocking `SyncSender::send` when the user-side channel is full. With
// the channel full and no reader, the worker never observes the `Die`
// command — it's just stuck in `send`. The only path out is dropping
// the user-side `Receiver`, which makes `send` return `SendError` and
// lets the worker fall through to the `Die` check.
//
// A regression that swapped the order — joining the worker before
// dropping the receiver (move the `self.frames = None` line to after
// `handle.join()`) — would leave `runner.stop()` (and `Drop`) hanging
// forever the moment a Block-mode runner is shut down without a
// drained channel. No existing test catches this:
// `runner_overflow_block_delivers_every_frame_in_order` actively
// drains the channel on the test thread, so the worker is never
// parked when shutdown runs;
// `runner_shutdown_drops_receivers_before_joining_drop_oldest_relay`
// pins the same invariant for `DropOldest` but exercises the relay
// thread, not the direct-`SyncSender::send` worker path.

#[test]
fn runner_shutdown_drops_receivers_before_joining_block_overflow_worker() {
    use nokhwa_core::types::CameraIndex;
    use std::time::Instant;

    let src = EndlessFrameSource {
        info: CameraInfo::new(
            "endless-block".to_string(),
            "endless-block".to_string(),
            "endless-block".to_string(),
            false,
            CameraIndex::Index(0),
        ),
        format: CameraFormat::new(
            nokhwa_core::types::Resolution::new(4, 4),
            FrameFormat::YUYV,
            30,
        ),
    };
    let opened = OpenedCamera::from_device(Box::new(src));
    let cfg = RunnerConfig {
        // Capacity 1 + Block forces the worker to park inside
        // `SyncSender::send` after the very first un-drained frame.
        frames_capacity: 1,
        overflow: Overflow::Block,
        ..RunnerConfig::default()
    };
    let runner = CameraRunner::spawn(opened, cfg).unwrap();

    // Let the worker push one frame and park on the next send. 100 ms
    // at 10 ms `poll_interval` is ~10 iterations — way past saturation.
    std::thread::sleep(Duration::from_millis(100));

    // Drop without ever draining. If `shutdown()` reverses the
    // documented ordering (joins the worker before dropping the
    // receiver), the worker stays parked in `SyncSender::send`, the
    // join never returns, and `runner.stop()` hangs forever.
    let stopper = std::thread::spawn(move || runner.stop());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !stopper.is_finished() {
        if Instant::now() >= deadline {
            panic!(
                "runner.stop() hung; Block-overflow worker likely \
                 parked in SyncSender::send because shutdown() joined \
                 the worker before dropping the user-facing receiver \
                 (src/runner.rs:534-540 ordering invariant regressed)"
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stopper.join().unwrap().unwrap();
}
