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

use crate::buffer::Buffer;
use crate::error::NokhwaError;
use crate::traits::{
    CameraDevice, CameraEvent, EventPoll, EventSource, FrameSource, HotplugEvent, ShutterCapture,
};
use crate::types::{
    ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
    FrameFormat, KnownCameraControl, Resolution,
};
use std::borrow::Cow;
use std::time::Duration;

struct Dummy {
    info: CameraInfo,
    open: bool,
}

impl CameraDevice for Dummy {
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

impl FrameSource for Dummy {
    fn negotiated_format(&self) -> CameraFormat {
        unimplemented!()
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
        self.open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        unimplemented!()
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Ok(Cow::Borrowed(&[]))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        self.open = false;
        Ok(())
    }
}

impl ShutterCapture for Dummy {
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn take_picture(&mut self, _timeout: Duration) -> Result<Buffer, NokhwaError> {
        Err(NokhwaError::TimeoutError(Duration::ZERO))
    }
}

struct DummyEvents;
impl EventPoll for DummyEvents {
    fn try_next(&mut self) -> Option<CameraEvent> {
        None
    }
    fn next_timeout(&mut self, _d: Duration) -> Option<CameraEvent> {
        None
    }
}

impl EventSource for Dummy {
    fn take_events(&mut self) -> Result<Box<dyn EventPoll + Send>, NokhwaError> {
        Ok(Box::new(DummyEvents))
    }
}

fn sample_info() -> CameraInfo {
    use crate::types::CameraIndex;
    CameraInfo::new(
        "dummy".to_string(),
        "dummy".to_string(),
        "".to_string(),
        false,
        CameraIndex::Index(0),
    )
}

#[test]
fn dummy_implements_all_capabilities() {
    let mut d = Dummy {
        info: sample_info(),
        open: false,
    };
    let _: &dyn CameraDevice = &d;
    let _: &mut dyn FrameSource = &mut d;
    let _: &mut dyn ShutterCapture = &mut d;
    let _: &mut dyn EventSource = &mut d;
}

#[test]
fn shutter_capture_default_methods() {
    let mut d = Dummy {
        info: sample_info(),
        open: false,
    };
    assert!(d.lock_ui().is_ok());
    assert!(d.unlock_ui().is_ok());
    let r = d.capture(Duration::ZERO);
    // The default `ShutterCapture::capture` (`traits.rs:277-282`) routes
    // through `Dummy::take_picture` (`traits_tests.rs:91-93`) which
    // hard-codes `TimeoutError(Duration::ZERO)`. Pin the payload — a
    // regression that wrapped the inner error in a different variant or
    // drifted the `Duration` would slip past the wildcard.
    assert!(matches!(r, Err(NokhwaError::TimeoutError(d)) if d == Duration::ZERO));
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // test stub: each bool toggles a distinct mock failure point
struct ShutterScript {
    lock_fail: bool,
    trigger_fail: bool,
    take_fail: bool,
    unlock_fail: bool,
    log: std::cell::RefCell<Vec<&'static str>>,
}

struct Scripted {
    info: CameraInfo,
    script: ShutterScript,
}

impl CameraDevice for Scripted {
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

impl FrameSource for Scripted {
    fn negotiated_format(&self) -> CameraFormat {
        unimplemented!()
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        false
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        unimplemented!()
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Ok(Cow::Borrowed(&[]))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

impl ShutterCapture for Scripted {
    fn lock_ui(&mut self) -> Result<(), NokhwaError> {
        self.script.log.borrow_mut().push("lock");
        if self.script.lock_fail {
            return Err(NokhwaError::general("lock-failed"));
        }
        Ok(())
    }
    fn unlock_ui(&mut self) -> Result<(), NokhwaError> {
        self.script.log.borrow_mut().push("unlock");
        if self.script.unlock_fail {
            return Err(NokhwaError::general("unlock-failed"));
        }
        Ok(())
    }
    fn trigger(&mut self) -> Result<(), NokhwaError> {
        self.script.log.borrow_mut().push("trigger");
        if self.script.trigger_fail {
            return Err(NokhwaError::general("trigger-failed"));
        }
        Ok(())
    }
    fn take_picture(&mut self, _timeout: Duration) -> Result<Buffer, NokhwaError> {
        self.script.log.borrow_mut().push("take");
        if self.script.take_fail {
            return Err(NokhwaError::general("take-failed"));
        }
        Ok(Buffer::new(
            Resolution::new(2, 2),
            &[0u8; 12],
            FrameFormat::RAWRGB,
        ))
    }
}

fn scripted(script: ShutterScript) -> Scripted {
    Scripted {
        info: sample_info(),
        script,
    }
}

#[test]
fn shutter_capture_lock_failure_short_circuits_before_trigger_and_unlock() {
    let mut s = scripted(ShutterScript {
        lock_fail: true,
        ..Default::default()
    });
    let r = s.capture(Duration::ZERO);
    // The `Scripted` fake at `traits_tests.rs:209` produces the error via
    // `NokhwaError::general("lock-failed")`, which always sets
    // `backend: None`. Pin both the message AND the absent backend so a
    // future refactor that reroutes through a backend-tagged constructor
    // (e.g. `GeneralError { backend: Some(...), .. }`) — silently changing
    // the public API shape and the log-grep target — is caught instead of
    // slipping past the `..` wildcard.
    assert!(
        matches!(
            &r,
            Err(NokhwaError::GeneralError { message, backend })
                if message == "lock-failed" && backend.is_none()
        ),
        "expected GeneralError {{ message: \"lock-failed\", backend: None }}, got {r:?}",
    );
    let log = s.script.log.borrow();
    assert_eq!(
        log.as_slice(),
        &["lock"],
        "lock failure must short-circuit before trigger/take/unlock",
    );
}

#[test]
fn shutter_capture_trigger_failure_skips_take_but_still_runs_unlock() {
    let mut s = scripted(ShutterScript {
        trigger_fail: true,
        ..Default::default()
    });
    let r = s.capture(Duration::ZERO);
    assert!(
        matches!(
            &r,
            Err(NokhwaError::GeneralError { message, backend })
                if message == "trigger-failed" && backend.is_none()
        ),
        "expected GeneralError {{ message: \"trigger-failed\", backend: None }}, got {r:?}",
    );
    let log = s.script.log.borrow();
    assert_eq!(
        log.as_slice(),
        &["lock", "trigger", "unlock"],
        "trigger failure must short-circuit before take_picture, but \
         `unlock_ui` must still run because `lock_ui` succeeded — the \
         lock cannot leak",
    );
}

#[test]
fn shutter_capture_take_failure_returns_take_error_and_runs_unlock() {
    let mut s = scripted(ShutterScript {
        take_fail: true,
        unlock_fail: true,
        ..Default::default()
    });
    let r = s.capture(Duration::ZERO);
    assert!(
        matches!(
            &r,
            Err(NokhwaError::GeneralError { message, backend })
                if message == "take-failed" && backend.is_none()
        ),
        "expected GeneralError {{ message: \"take-failed\", backend: None }}; \
         take_picture's error must be the one returned, never unlock_ui's; \
         got {r:?}",
    );
    let log = s.script.log.borrow();
    assert_eq!(
        log.as_slice(),
        &["lock", "trigger", "take", "unlock"],
        "unlock_ui must always run after take_picture, even when both fail",
    );
}

#[test]
fn shutter_capture_unlock_error_is_silently_discarded_on_success() {
    let mut s = scripted(ShutterScript {
        unlock_fail: true,
        ..Default::default()
    });
    let r = s.capture(Duration::ZERO);
    assert!(
        r.is_ok(),
        "unlock_ui's error must be discarded so a successful capture stays Ok; \
         got {r:?}",
    );
    let log = s.script.log.borrow();
    assert_eq!(log.as_slice(), &["lock", "trigger", "take", "unlock"]);
}

struct FormatStub {
    info: CameraInfo,
    fmt: CameraFormat,
}

impl CameraDevice for FormatStub {
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

impl FrameSource for FormatStub {
    fn negotiated_format(&self) -> CameraFormat {
        self.fmt
    }
    fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
        self.fmt = f;
        Ok(())
    }
    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        Ok(vec![])
    }
    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        Ok(vec![])
    }
    fn open(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
    fn is_open(&self) -> bool {
        false
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        unimplemented!()
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Ok(Cow::Borrowed(&[]))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

fn stub(format: FrameFormat, w: u32, h: u32) -> FormatStub {
    FormatStub {
        info: sample_info(),
        fmt: CameraFormat::new(Resolution::new(w, h), format, 30),
    }
}

#[test]
fn decoded_buffer_size_three_byte_formats_alpha_off() {
    for f in [
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
        FrameFormat::NV12,
    ] {
        let s = stub(f, 640, 480);
        assert_eq!(
            s.decoded_buffer_size(false),
            640 * 480 * 3,
            "format {f:?} alpha=false"
        );
    }
}

#[test]
fn decoded_buffer_size_three_byte_formats_alpha_on() {
    for f in [
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
        FrameFormat::NV12,
    ] {
        let s = stub(f, 320, 240);
        assert_eq!(
            s.decoded_buffer_size(true),
            320 * 240 * 4,
            "format {f:?} alpha=true"
        );
    }
}

#[test]
fn decoded_buffer_size_gray_alpha_off_is_one_byte_per_pixel() {
    let s = stub(FrameFormat::GRAY, 1920, 1080);
    assert_eq!(s.decoded_buffer_size(false), 1920 * 1080);
}

#[test]
fn decoded_buffer_size_gray_alpha_on_is_four_bytes_per_pixel() {
    // GRAY+alpha decodes to RGBA: 4 bpp, not 2 (the old pxwidth+1 formula
    // was wrong for GRAY because decoded_pixel_byte_width() returns 1 for
    // GRAY, making pxwidth+1=2 instead of 4).
    let s = stub(FrameFormat::GRAY, 1920, 1080);
    assert_eq!(s.decoded_buffer_size(true), 1920 * 1080 * 4);
}

#[test]
fn decoded_buffer_size_gray_alpha_on_smallest_resolution_is_four_bytes() {
    // 1×1 GRAY+alpha must be 4 bytes (one RGBA pixel), not 2. This case
    // previously validated the bug: pxwidth+1 = 1+1 = 2 for GRAY while
    // convert_to_rgba actually produces w·h·4 bytes.
    let s = stub(FrameFormat::GRAY, 1, 1);
    assert_eq!(s.decoded_buffer_size(true), 4);
}

#[test]
fn decoded_buffer_size_zero_resolution_returns_zero_for_every_format() {
    // 0×0 must produce 0 bytes for every `FrameFormat`, regardless of
    // alpha. A bug that returned `pxwidth` (or `pxwidth + 1`) for a
    // zero-resolution frame would feed a non-zero allocation request
    // into `frame_texture` for a buffer that has zero actual frame
    // bytes — a silent memory hazard at the wgpu upload boundary.
    for f in [
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
        FrameFormat::GRAY,
    ] {
        let s = stub(f, 0, 0);
        assert_eq!(s.decoded_buffer_size(false), 0, "{f:?} alpha=false");
        assert_eq!(s.decoded_buffer_size(true), 0, "{f:?} alpha=true");
    }
}

struct FrameCallCounter {
    info: CameraInfo,
    fmt: CameraFormat,
    frame_calls: u32,
}

impl CameraDevice for FrameCallCounter {
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

impl FrameSource for FrameCallCounter {
    fn negotiated_format(&self) -> CameraFormat {
        self.fmt
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
        Ok(())
    }
    fn is_open(&self) -> bool {
        false
    }
    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.frame_calls += 1;
        Ok(Buffer::new(self.fmt.resolution(), &[], self.fmt.format()))
    }
    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        Ok(Cow::Borrowed(&[]))
    }
    fn close(&mut self) -> Result<(), NokhwaError> {
        Ok(())
    }
}

#[test]
fn frame_timeout_default_forwards_to_frame() {
    let mut c = FrameCallCounter {
        info: sample_info(),
        fmt: CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30),
        frame_calls: 0,
    };
    let _ = c.frame_timeout(Duration::from_millis(50));
    assert_eq!(c.frame_calls, 1);
    let _ = c.frame_timeout(Duration::from_millis(50));
    assert_eq!(c.frame_calls, 2);
}

// ─────────────── HotplugEvent + CameraEvent derive contracts ──────────
//
// `HotplugEvent` derives `Clone, PartialEq, Eq, Hash, Debug`. The trait
// docs explicitly direct callers to dedup events via `HashSet` / hashmap
// keys and to match Connected / Disconnected pairs by
// `CameraInfo::index()`. A regression that drops or weakens any of those
// derives would silently break that contract for every downstream
// hotplug consumer. `CameraEvent` derives `Clone, Debug` only (no Eq —
// `CaptureError { code, message }` carries a `String`); pin those.

fn make_info(idx: u32, name: &str) -> CameraInfo {
    CameraInfo::new(
        name.to_string(),
        "desc".to_string(),
        "misc".to_string(),
        false,
        CameraIndex::Index(idx),
    )
}

#[test]
fn hotplug_event_partial_eq_compares_full_camera_info() {
    let a = HotplugEvent::Connected(make_info(0, "Cam"));
    let b = HotplugEvent::Connected(make_info(0, "Cam"));
    let c = HotplugEvent::Connected(make_info(0, "OtherName"));
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn hotplug_event_connected_and_disconnected_with_same_info_are_distinct() {
    let info = make_info(0, "Cam");
    let connected = HotplugEvent::Connected(info.clone());
    let disconnected = HotplugEvent::Disconnected(info);
    assert_ne!(connected, disconnected);
}

#[test]
fn hotplug_event_clone_round_trip_preserves_info() {
    let info = make_info(7, "Cam");
    let ev = HotplugEvent::Disconnected(info);
    let cloned = ev.clone();
    assert_eq!(ev, cloned);
    if let HotplugEvent::Disconnected(c) = cloned {
        assert_eq!(c.index(), &CameraIndex::Index(7));
        assert_eq!(c.human_name(), "Cam");
    } else {
        panic!("clone changed variant");
    }
}

#[test]
fn hotplug_event_dedups_in_hashset() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(HotplugEvent::Connected(make_info(0, "Cam")));
    set.insert(HotplugEvent::Connected(make_info(0, "Cam")));
    set.insert(HotplugEvent::Disconnected(make_info(0, "Cam")));
    assert_eq!(set.len(), 2);
}

#[test]
fn hotplug_event_index_matches_across_connect_disconnect_with_drift() {
    // Documented contract: "best-effort" name/desc/misc may drift between
    // Connected and Disconnected; consumers should match on `index()`.
    let connected = HotplugEvent::Connected(make_info(3, "Cam Original"));
    let disconnected = HotplugEvent::Disconnected(make_info(3, "Cam Drifted"));
    let connected_idx = match &connected {
        HotplugEvent::Connected(i) | HotplugEvent::Disconnected(i) => i.index().clone(),
    };
    let disconnected_idx = match &disconnected {
        HotplugEvent::Connected(i) | HotplugEvent::Disconnected(i) => i.index().clone(),
    };
    assert_eq!(connected_idx, disconnected_idx);
    // Structural equality must still differ because name drifted.
    assert_ne!(connected, disconnected);
}

// Hardened from a contains-only assertion. The derived `Debug` for
// `HotplugEvent` plus `CameraInfo` is the documented dedupe / log
// surface used by hotplug consumers (see `HotplugSource` docs);
// pinning the exact format catches a future hand-written `impl Debug`
// that, say, dropped the `description` / `misc` fields to "tidy up"
// log output and silently broke any tooling grep-ing those fields.
#[test]
fn hotplug_event_debug_exact_format() {
    let info = CameraInfo::new(
        "Cam".to_string(),
        "desc".to_string(),
        "misc".to_string(),
        false,
        CameraIndex::Index(0),
    );
    assert_eq!(
        format!("{:?}", HotplugEvent::Connected(info.clone())),
        "Connected(CameraInfo { human_name: \"Cam\", description: \"desc\", \
         misc: \"misc\", index: Index(0) })"
    );
    assert_eq!(
        format!("{:?}", HotplugEvent::Disconnected(info)),
        "Disconnected(CameraInfo { human_name: \"Cam\", description: \"desc\", \
         misc: \"misc\", index: Index(0) })"
    );
}

#[test]
fn camera_event_clone_preserves_capture_error_fields() {
    let ev = CameraEvent::CaptureError {
        code: -42,
        message: "boom".to_string(),
    };
    let cloned = ev.clone();
    if let CameraEvent::CaptureError { code, message } = cloned {
        assert_eq!(code, -42);
        assert_eq!(message, "boom");
    } else {
        panic!("clone changed variant");
    }
}

// Hardened from contains-only checks: pin the full derived-Debug
// format for every `CameraEvent` variant including the `code` /
// `message` field names of `CaptureError`. The previous test would
// pass even if a hand-written `impl Debug` collapsed
// `CaptureError { code, message }` to a tuple-style
// `CaptureError(-42, "boom")` (a tempting "less verbose" rewrite),
// silently breaking log-shape contracts and any grep-based filters
// used by event-stream consumers.
#[test]
fn camera_event_debug_exact_format() {
    assert_eq!(format!("{:?}", CameraEvent::Disconnected), "Disconnected");
    assert_eq!(format!("{:?}", CameraEvent::WillShutDown), "WillShutDown");
    assert_eq!(
        format!(
            "{:?}",
            CameraEvent::CaptureError {
                code: -42,
                message: "boom".to_string(),
            }
        ),
        "CaptureError { code: -42, message: \"boom\" }"
    );
}
