//! Backend-level hotplug for the `AVFoundation` backend.
//!
//! Event-driven: a dedicated worker thread runs a `CFRunLoop` with
//! `IOServiceAddMatchingNotification` watching for `kIOFirstMatchNotification`
//! / `kIOTerminatedNotification` on `IOUSBHostDevice`. Steady-state CPU is
//! zero — the OS parks the thread until USB topology actually changes.
//!
//! Why match on `IOUSBHostDevice` and not on a camera-specific class: there
//! is no `IOKit` class that captures every `AVFoundation` device (USB UVC,
//! Continuity Camera, virtual cams from system extensions, etc.) in a way
//! that `AVFoundation` enumerates. So we use `IOUSBHostDevice` as a *trigger*
//! — when *anything* USB plugs or unplugs we re-run `device::query()` and
//! diff. Non-USB hotplug paths (Continuity Camera handoff) are rare and
//! `EventSource` still surfaces them via the `AVFoundation` per-device
//! notifications. The 500 ms latency floor of the old polling impl is gone
//! for USB cameras (the dominant case) and unchanged for the rest.
//!
//! Diff logic is unchanged from the polling version — `reconcile_and_emit_with`
//! is what the unit tests pin.

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod real {
    use crate::device::query;
    use nokhwa_core::{
        error::NokhwaError,
        traits::{HotplugEvent, HotplugEventPoll, HotplugSource},
        types::{ApiBackend, CameraInfo},
    };
    use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRetained, CFRunLoop};
    use objc2_io_kit::{
        io_iterator_t, kIOFirstMatchNotification, kIOMainPortDefault, kIOTerminatedNotification,
        IOIteratorNext, IONotificationPort, IOObjectRelease, IOServiceAddMatchingNotification,
        IOServiceMatching,
    };
    use std::{
        collections::BTreeMap,
        ffi::c_void,
        sync::{
            atomic::{AtomicBool, AtomicPtr, Ordering},
            mpsc::{self, Receiver, Sender},
            Arc,
        },
        thread::{self, JoinHandle},
        time::Duration,
    };

    /// Backend-level hotplug source for `AVFoundation`. Cheap to
    /// construct — the worker thread is only spawned when
    /// [`take_hotplug_events`](HotplugSource::take_hotplug_events) is
    /// called, and is joined when the returned poller is dropped.
    #[derive(Default)]
    pub struct AVFoundationHotplugContext {
        taken: bool,
    }

    impl AVFoundationHotplugContext {
        #[must_use]
        pub fn new() -> Self {
            Self { taken: false }
        }
    }

    impl HotplugSource for AVFoundationHotplugContext {
        fn take_hotplug_events(&mut self) -> Result<Box<dyn HotplugEventPoll + Send>, NokhwaError> {
            if self.taken {
                return Err(NokhwaError::UnsupportedOperationError(
                    ApiBackend::AVFoundation,
                ));
            }
            self.taken = true;
            Ok(Box::new(AvfHotplugPoll::spawn()?))
        }
    }

    /// Concrete [`HotplugEventPoll`]. Owns a worker thread that drives a
    /// `CFRunLoop` listening to `IOKit` matching notifications, plus an
    /// mpsc channel for delivered events. Dropping the poll stops the
    /// runloop and joins the thread.
    struct AvfHotplugPoll {
        rx: Receiver<HotplugEvent>,
        stop: Arc<AtomicBool>,
        // The worker thread publishes its `CFRunLoopRef` here so `Drop`
        // can wake it with `CFRunLoopStop`. The raw pointer is stored
        // because `CFRunLoop` itself is `!Send` (it represents a
        // thread-local loop), but `CFRunLoopStop` is documented as
        // thread-safe — calling it from another thread to wake a
        // blocked runloop is the canonical pattern. Null if the worker
        // has not yet published or has already exited.
        runloop_ptr: Arc<AtomicPtr<CFRunLoop>>,
        join: Option<JoinHandle<()>>,
    }

    impl AvfHotplugPoll {
        fn spawn() -> Result<Self, NokhwaError> {
            let (tx, rx) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let runloop_ptr: Arc<AtomicPtr<CFRunLoop>> =
                Arc::new(AtomicPtr::new(std::ptr::null_mut()));

            let stop_t = Arc::clone(&stop);
            let rl_t = Arc::clone(&runloop_ptr);
            let join = thread::Builder::new()
                .name("nokhwa-avf-hotplug".to_string())
                .spawn(move || worker(tx, &stop_t, &rl_t))
                .map_err(|e| NokhwaError::general(format!("spawn hotplug thread: {e}")))?;
            Ok(Self {
                rx,
                stop,
                runloop_ptr,
                join: Some(join),
            })
        }
    }

    impl HotplugEventPoll for AvfHotplugPoll {
        fn try_next(&mut self) -> Option<HotplugEvent> {
            self.rx.try_recv().ok()
        }
        fn next_timeout(&mut self, d: Duration) -> Option<HotplugEvent> {
            self.rx.recv_timeout(d).ok()
        }
    }

    impl Drop for AvfHotplugPoll {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            // Wake the worker's CFRunLoop so it observes the stop flag
            // and returns. `CFRunLoopStop` is thread-safe per Apple's
            // CFRunLoop documentation — that's the whole reason we can
            // cross-thread stop a runloop from Drop.
            let rl_raw = self.runloop_ptr.load(Ordering::Acquire);
            if !rl_raw.is_null() {
                // SAFETY: The worker keeps the CFRunLoop alive (it owns
                // a `CFRetained<CFRunLoop>`) until it returns from
                // `CFRunLoopRun`. The runloop_ptr is published before
                // `run()` and cleared on the worker side only after
                // run() returns, so as long as we observe non-null
                // here, the pointer is still valid.
                unsafe {
                    (*rl_raw).stop();
                }
            }
            if let Some(h) = self.join.take() {
                let _ = h.join();
            }
        }
    }

    /// State held inside the `IOKit` callback. The callback is an
    /// `extern "C"` function with a `*mut c_void` user-context pointer,
    /// so we leak a `Box` containing this struct on the worker thread,
    /// hand the raw pointer to `IOKit`, and reclaim it on shutdown.
    struct CbCtx {
        tx: Sender<HotplugEvent>,
        previous: BTreeMap<String, CameraInfo>,
        channel_dead: bool,
    }

    /// `IOKit` callback for both first-match and terminated notifications.
    /// `IOKit` *requires* draining the iterator (otherwise the
    /// notification stays unarmed and never fires again), then re-enums
    /// `AVFoundation` devices and diffs.
    extern "C-unwind" fn matching_cb(refcon: *mut c_void, iter: io_iterator_t) {
        // Drain the iterator — required to re-arm the notification.
        loop {
            let svc = IOIteratorNext(iter);
            if svc == 0 {
                break;
            }
            IOObjectRelease(svc);
        }
        // SAFETY: `refcon` was set to a `Box::into_raw(Box<CbCtx>)` on
        // the worker thread; it stays valid until the worker tears down
        // the notification port and reclaims the box.
        let ctx = unsafe { &mut *(refcon.cast::<CbCtx>()) };
        if ctx.channel_dead {
            return;
        }
        let current = snapshot();
        if !reconcile_and_emit_with(&ctx.tx, &mut ctx.previous, current) {
            ctx.channel_dead = true;
        }
    }

    /// Build a `CFDictionary` matching `IOUSBHostDevice` — coerces the
    /// `IOServiceMatching` result (`CFMutableDictionary`) to the
    /// `CFDictionary` view `IOKit`'s notification API expects.
    fn matching_usb() -> Option<CFRetained<objc2_core_foundation::CFDictionary>> {
        // SAFETY: `IOServiceMatching` accepts a NUL-terminated C string and
        // returns a retained `CFMutableDictionary` (or null). We narrow to
        // the immutable `CFDictionary` view via raw-pointer cast, which is
        // a no-op since `CFMutableDictionary` is a subtype.
        unsafe {
            let mutable = IOServiceMatching(c"IOUSBHostDevice".as_ptr())?;
            let raw = CFRetained::into_raw(mutable).cast::<objc2_core_foundation::CFDictionary>();
            Some(CFRetained::from_raw(raw))
        }
    }

    /// Worker thread body. Sets up two matching notifications (arrive +
    /// terminate), publishes its `CFRunLoop` so `Drop` can wake it, then
    /// runs the runloop. On exit, cleans up `IOKit` handles and reclaims
    /// the boxed callback context.
    fn worker(
        tx: Sender<HotplugEvent>,
        stop: &Arc<AtomicBool>,
        runloop_slot: &Arc<AtomicPtr<CFRunLoop>>,
    ) {
        // SAFETY: All IOKit / CoreFoundation calls below follow the
        // standard "create → register → run → unregister → destroy"
        // lifecycle for matching notifications. Pointers we hand to
        // IOKit (notification port, callback refcon) are kept alive on
        // this stack frame for the duration of the runloop, and freed
        // *after* the port is destroyed.
        unsafe {
            let port = IONotificationPort::create(kIOMainPortDefault);
            if port.is_null() {
                // Can't seed the diff cache via a callback; fall back to
                // letting the consumer time out. Channel still works.
                return;
            }

            let Some(run_source) = IONotificationPort::run_loop_source(port) else {
                IONotificationPort::destroy(port);
                return;
            };

            let Some(rl) = CFRunLoop::current() else {
                IONotificationPort::destroy(port);
                return;
            };
            rl.add_source(Some(&run_source), kCFRunLoopDefaultMode);

            // Publish the runloop pointer so Drop on the parent side
            // can wake us. `CFRetained::as_ptr` returns a borrowed raw
            // pointer that stays valid as long as `rl` is in scope —
            // we keep `rl` alive until after `CFRunLoopRun` returns.
            // `rl_raw` itself never escapes this function (only the
            // pointer value does, via AtomicPtr).
            let rl_raw: *mut CFRunLoop = CFRetained::as_ptr(&rl).as_ptr();
            runloop_slot.store(rl_raw, Ordering::Release);

            let ctx = Box::new(CbCtx {
                tx,
                previous: snapshot(),
                channel_dead: false,
            });
            let ctx_raw: *mut CbCtx = Box::into_raw(ctx);

            let mut iter_match: io_iterator_t = 0;
            let mut iter_term: io_iterator_t = 0;

            // Register arrive + terminate. We use one shared callback;
            // the diff logic figures out what changed by re-enumerating.
            let kr1 = IOServiceAddMatchingNotification(
                port,
                kIOFirstMatchNotification.as_ptr().cast_mut().cast(),
                matching_usb(),
                Some(matching_cb),
                ctx_raw.cast::<c_void>(),
                &raw mut iter_match,
            );
            let kr2 = IOServiceAddMatchingNotification(
                port,
                kIOTerminatedNotification.as_ptr().cast_mut().cast(),
                matching_usb(),
                Some(matching_cb),
                ctx_raw.cast::<c_void>(),
                &raw mut iter_term,
            );

            // Either notification failing leaves us in a degraded but
            // safe state: still drain whatever did register so the
            // notification is armed, then keep running so Drop's join
            // succeeds. (Returning early would leave the parent stuck
            // in `recv_timeout` until the channel closes on its own.)
            if kr1 == 0 {
                drain(iter_match);
            }
            if kr2 == 0 {
                drain(iter_term);
            }

            // Block the worker until Drop calls CFRunLoopStop. If the
            // stop flag is already set (Drop races with spawn) we skip
            // straight to teardown.
            if !stop.load(Ordering::Acquire) {
                CFRunLoop::run();
            }

            // Teardown order: clear the runloop publication first so a
            // late Drop doesn't try to stop an already-stopping loop;
            // remove sources by destroying the port; release iterator
            // handles; then reclaim the box.
            runloop_slot.store(std::ptr::null_mut(), Ordering::Release);
            if iter_match != 0 {
                IOObjectRelease(iter_match);
            }
            if iter_term != 0 {
                IOObjectRelease(iter_term);
            }
            IONotificationPort::destroy(port);
            // Reclaim and drop the callback context. Safe because IOKit
            // has been told to stop calling `matching_cb` (port
            // destroyed) and the runloop has returned.
            drop(Box::from_raw(ctx_raw));
        }
    }

    /// Drain an `IOKit` iterator and release each service handle.
    /// Required after registering a matching notification, otherwise
    /// the notification stays unarmed.
    fn drain(iter: io_iterator_t) {
        loop {
            let svc = IOIteratorNext(iter);
            if svc == 0 {
                break;
            }
            IOObjectRelease(svc);
        }
    }

    /// Diff `current` against `previous`, emit events, swap cache.
    /// Returns false if the channel is closed (consumer dropped the
    /// poller) so the worker can shut down early. Split out so unit
    /// tests can inject a synthetic `current` without touching `IOKit` or
    /// `AVFoundation`.
    ///
    /// Emit arrivals before removals so a rapid re-plug landing in one
    /// callback batch surfaces as `Disconnected` → `Connected` on the
    /// consumer side.
    fn reconcile_and_emit_with(
        tx: &Sender<HotplugEvent>,
        previous: &mut BTreeMap<String, CameraInfo>,
        current: BTreeMap<String, CameraInfo>,
    ) -> bool {
        for (key, info) in &current {
            if !previous.contains_key(key)
                && tx.send(HotplugEvent::Connected(info.clone())).is_err()
            {
                return false;
            }
        }
        for (key, info) in previous.iter() {
            if !current.contains_key(key)
                && tx.send(HotplugEvent::Disconnected(info.clone())).is_err()
            {
                return false;
            }
        }
        *previous = current;
        true
    }

    /// One `AVFoundation` enumeration pass, indexed by
    /// `AVCaptureDevice.uniqueID` (stored in `CameraInfo.misc` by the
    /// device module). `uniqueID` is stable across enumerations for a
    /// given physical device and does not repeat across ports, so it
    /// is the right diff key — same shape as the MSMF symbolic-link
    /// diff.
    ///
    /// Errors from `query()` are swallowed — a transient enumeration
    /// failure should not tear down the hotplug thread. An empty
    /// snapshot will look like "every device disappeared"; next
    /// callback we will re-emit them as `Connected`. That is noisy but
    /// not incorrect.
    fn snapshot() -> BTreeMap<String, CameraInfo> {
        match query() {
            Ok(list) => list.into_iter().map(|ci| (ci.misc(), ci)).collect(),
            Err(_) => BTreeMap::new(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::reconcile_and_emit_with;
        use nokhwa_core::{
            traits::HotplugEvent,
            types::{CameraIndex, CameraInfo},
        };
        use std::{collections::BTreeMap, sync::mpsc};

        fn info(idx: u32) -> CameraInfo {
            CameraInfo::new(
                format!("cam{idx}"),
                "test".to_string(),
                format!("0x{idx:016x}-AVCaptureDevice-uniqueID"),
                false,
                CameraIndex::Index(idx),
            )
        }

        fn snap<I: IntoIterator<Item = u32>>(ids: I) -> BTreeMap<String, CameraInfo> {
            ids.into_iter()
                .map(|i| {
                    let ci = info(i);
                    (ci.misc(), ci)
                })
                .collect()
        }

        /// `reconcile_and_emit_with` must replace `previous` with
        /// `current` so the next call sees the updated cache.
        #[test]
        fn cache_is_swapped_after_reconcile() {
            let (tx, _rx) = mpsc::channel();
            let mut previous = snap([0]);
            let current = snap([1, 2]);
            assert!(reconcile_and_emit_with(&tx, &mut previous, current));
            assert_eq!(previous.len(), 2);
            assert!(previous.values().any(|ci| ci.human_name() == "cam1"));
            assert!(previous.values().any(|ci| ci.human_name() == "cam2"));
        }

        /// Newcomers (in `current`, not in `previous`) emit
        /// `Connected`; removals (in `previous`, not in `current`)
        /// emit `Disconnected`.
        #[test]
        fn arrivals_and_removals_are_both_emitted() {
            let (tx, rx) = mpsc::channel();
            let mut previous = snap([0, 1]);
            let current = snap([1, 2]);
            assert!(reconcile_and_emit_with(&tx, &mut previous, current));
            drop(tx);
            let events: Vec<_> = rx.iter().collect();
            assert_eq!(events.len(), 2, "got: {events:?}");
            let connected = events
                .iter()
                .filter(|e| matches!(e, HotplugEvent::Connected(_)))
                .count();
            let disconnected = events
                .iter()
                .filter(|e| matches!(e, HotplugEvent::Disconnected(_)))
                .count();
            assert_eq!(connected, 1, "expected 1 Connected, got {events:?}");
            assert_eq!(disconnected, 1, "expected 1 Disconnected, got {events:?}");
        }

        /// Pin the documented ordering invariant: arrivals are sent
        /// before removals so a re-plug landing in one callback batch
        /// is observable as `Disconnected` → `Connected` on the
        /// consumer side.
        #[test]
        fn arrivals_precede_removals_in_emission_order() {
            let (tx, rx) = mpsc::channel();
            let mut previous = snap([0]);
            let current = snap([1]);
            assert!(reconcile_and_emit_with(&tx, &mut previous, current));
            drop(tx);
            let events: Vec<_> = rx.iter().collect();
            assert_eq!(events.len(), 2);
            assert!(
                matches!(events[0], HotplugEvent::Connected(_)),
                "first event must be Connected, got {:?}",
                events[0]
            );
            assert!(
                matches!(events[1], HotplugEvent::Disconnected(_)),
                "second event must be Disconnected, got {:?}",
                events[1]
            );
        }

        /// No-op reconcile (current == previous) emits zero events
        /// and leaves the cache equal.
        #[test]
        fn identical_snapshots_emit_no_events() {
            let (tx, rx) = mpsc::channel();
            let mut previous = snap([0, 1, 2]);
            let current = snap([0, 1, 2]);
            assert!(reconcile_and_emit_with(&tx, &mut previous, current));
            drop(tx);
            assert_eq!(rx.iter().count(), 0);
            assert_eq!(previous.len(), 3);
        }

        /// If the channel is closed mid-emission, return false so
        /// the worker can exit early instead of looping over a dead
        /// channel.
        #[test]
        fn returns_false_when_channel_closed() {
            let (tx, rx) = mpsc::channel();
            drop(rx);
            let mut previous = snap([0]);
            let current = snap([1]);
            assert!(!reconcile_and_emit_with(&tx, &mut previous, current));
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod real {
    use nokhwa_core::{
        error::NokhwaError,
        traits::{HotplugEventPoll, HotplugSource},
        types::ApiBackend,
    };

    /// Non-Apple stub for [`AVFoundationHotplugContext`]. Every method
    /// errors with [`NokhwaError::UnsupportedOperationError`].
    #[derive(Default)]
    pub struct AVFoundationHotplugContext;

    impl AVFoundationHotplugContext {
        #[must_use]
        pub fn new() -> Self {
            Self
        }
    }

    impl HotplugSource for AVFoundationHotplugContext {
        fn take_hotplug_events(&mut self) -> Result<Box<dyn HotplugEventPoll + Send>, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::AVFoundation,
            ))
        }
    }
}

pub use real::AVFoundationHotplugContext;
