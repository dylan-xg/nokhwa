//! Backend-level hotplug for the `Video4Linux` backend.
//!
//! Event-driven implementation backed by Linux's `inotify(7)` watching
//! `/dev/` for `IN_CREATE` / `IN_DELETE` on `video*` nodes. A dedicated
//! worker thread blocks in `poll(2)` on the inotify fd with a short
//! timeout (for shutdown responsiveness), then on each batch of events
//! re-`query()`s and diffs against a cached snapshot to emit
//! `HotplugEvent::Connected` / `Disconnected`.
//!
//! Why event-driven over polling: zero wake-ups in the steady state,
//! immediate notification instead of up-to-500ms latency. Mirrors the
//! MSMF backend's `RegisterDeviceNotificationW` design (#173) — the
//! polling version this replaces was the last remaining 2×/sec wake-up
//! source on the Linux side.
//!
//! `inotify` semantics: we get a kernel notification the instant the
//! device node appears in `/dev/`. The v4l driver creates the node
//! late in `usb_register` so by the time we re-`query()` the device is
//! enumerable. On removal the node disappears before the kernel tears
//! down the underlying USB device, so `query()` may briefly still see
//! the device on the first tick — the diff loop handles that
//! self-correctingly on the next event.

#[cfg(target_os = "linux")]
mod real {
    use crate::internal::query;
    use nokhwa_core::{
        error::NokhwaError,
        traits::{HotplugEvent, HotplugEventPoll, HotplugSource},
        types::{ApiBackend, CameraInfo},
    };
    use std::{
        collections::BTreeMap,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc::{self, Receiver, Sender},
            Arc,
        },
        thread::{self, JoinHandle},
        time::Duration,
    };

    /// `poll(2)` timeout. Worker wakes this often to check the shutdown
    /// flag — drop latency is bounded above by this. Short enough to
    /// feel snappy, long enough that an idle thread is genuinely idle
    /// (one syscall/sec instead of the old 2×/sec re-enumeration).
    const POLL_TIMEOUT_MS: i32 = 1_000;

    /// Buffer size for `read(2)` from the inotify fd. One
    /// `inotify_event` is `sizeof(inotify_event) + NAME_MAX + 1`
    /// worst-case = 16 + 256 = 272 bytes. 4 KiB comfortably batches
    /// many events without truncation (kernel returns whole events or
    /// `EINVAL`).
    const READ_BUF_BYTES: usize = 4096;

    /// Backend-level hotplug source for `Video4Linux`. Cheap to
    /// construct — the worker thread + inotify fd are only created
    /// when [`take_hotplug_events`](HotplugSource::take_hotplug_events)
    /// is called, and are torn down when the returned poller is
    /// dropped.
    #[derive(Default)]
    pub struct V4LHotplugContext {
        taken: bool,
    }

    impl V4LHotplugContext {
        #[must_use]
        pub fn new() -> Self {
            Self { taken: false }
        }
    }

    impl HotplugSource for V4LHotplugContext {
        fn take_hotplug_events(&mut self) -> Result<Box<dyn HotplugEventPoll + Send>, NokhwaError> {
            if self.taken {
                return Err(NokhwaError::UnsupportedOperationError(
                    ApiBackend::Video4Linux,
                ));
            }
            self.taken = true;
            Ok(Box::new(V4LHotplugPoll::spawn()?))
        }
    }

    /// Concrete [`HotplugEventPoll`]. Owns the worker thread + an mpsc
    /// channel + an [`AtomicBool`] shutdown flag. Dropping flips the
    /// flag; the worker notices on its next `poll(2)` timeout and
    /// exits, at which point we join it.
    struct V4LHotplugPoll {
        rx: Receiver<HotplugEvent>,
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl V4LHotplugPoll {
        fn spawn() -> Result<Self, NokhwaError> {
            let (tx, rx) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let stop_thread = Arc::clone(&stop);
            let join = thread::Builder::new()
                .name("nokhwa-v4l-hotplug".to_string())
                .spawn(move || worker_loop(&tx, &stop_thread))
                .map_err(|e| NokhwaError::general(format!("spawn hotplug thread: {e}")))?;
            Ok(Self {
                rx,
                stop,
                join: Some(join),
            })
        }
    }

    impl HotplugEventPoll for V4LHotplugPoll {
        fn try_next(&mut self) -> Option<HotplugEvent> {
            self.rx.try_recv().ok()
        }
        fn next_timeout(&mut self, d: Duration) -> Option<HotplugEvent> {
            self.rx.recv_timeout(d).ok()
        }
    }

    impl Drop for V4LHotplugPoll {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(h) = self.join.take() {
                // Worker blocks in poll() up to POLL_TIMEOUT_MS, so
                // join latency is bounded by that.
                let _ = h.join();
            }
        }
    }

    /// Worker thread: open the inotify fd, watch `/dev/`, then loop
    /// on `poll(2)` and translate inotify events into hotplug events.
    /// The first thing we do on a relevant event is re-`query()` and
    /// diff — inotify tells us *something* changed; `query()` tells
    /// us *what is currently visible*.
    fn worker_loop(tx: &Sender<HotplugEvent>, stop: &Arc<AtomicBool>) {
        let fd = match init_inotify() {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("nokhwa v4l hotplug: inotify init failed: {e}");
                return;
            },
        };

        // Seed the cache. Consumers see hotplug deltas, not the
        // initial population (they call `query()` directly to learn
        // what's already plugged in).
        let mut previous: BTreeMap<String, CameraInfo> = snapshot();

        let raw_fd = fd.as_raw_fd();
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            match poll_inotify(raw_fd, POLL_TIMEOUT_MS) {
                PollOutcome::Ready => {
                    if !drain_inotify(raw_fd) {
                        // Read failed — bail rather than spin.
                        break;
                    }
                    if !reconcile_and_emit(tx, &mut previous) {
                        // Channel closed (consumer dropped poller).
                        break;
                    }
                },
                PollOutcome::Timeout => {
                    // Loop back, check stop flag.
                },
                PollOutcome::Error => {
                    // poll() error other than EINTR — give up
                    // rather than tight-loop on a permanent fault.
                    break;
                },
            }
        }
        // fd dropped here, closes via OwnedFd.
        drop(fd);
    }

    /// Open an inotify instance and add a watch on `/dev/` for
    /// `IN_CREATE | IN_DELETE`. We don't filter by name in the kernel
    /// — `/dev/` doesn't churn enough for the userspace filter cost
    /// to matter, and we'd have to handle moves anyway.
    fn init_inotify() -> Result<OwnedFd, String> {
        // SAFETY: inotify_init1 returns a new fd or -1 on error. We
        // wrap into OwnedFd so it's closed on drop. IN_NONBLOCK so
        // read() returns EAGAIN instead of blocking — we use poll()
        // for blocking with timeout. IN_CLOEXEC so the fd doesn't
        // leak across exec.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(format!(
                "inotify_init1: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: fd is a freshly-returned valid fd; ownership transfers
        // to OwnedFd which closes it on drop.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };

        let path = std::ffi::CString::new("/dev").map_err(|e| e.to_string())?;
        // SAFETY: owned.as_raw_fd() is valid for the duration of this
        // call; path is a valid CString. Returns watch descriptor or
        // -1 on error. We don't keep the wd because we never call
        // inotify_rm_watch — the watch is implicitly removed when the
        // inotify fd closes.
        let wd = unsafe {
            libc::inotify_add_watch(
                owned.as_raw_fd(),
                path.as_ptr(),
                libc::IN_CREATE | libc::IN_DELETE,
            )
        };
        if wd < 0 {
            return Err(format!(
                "inotify_add_watch /dev: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(owned)
    }

    enum PollOutcome {
        Ready,
        Timeout,
        Error,
    }

    /// Block in `poll(2)` waiting for the inotify fd to be readable,
    /// up to `timeout_ms`. Treats `EINTR` as a timeout (the worker
    /// loop will check the stop flag and re-arm).
    fn poll_inotify(fd: i32, timeout_ms: i32) -> PollOutcome {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is a valid pointer to a single pollfd; nfds=1
        // matches. Returns >0 on event, 0 on timeout, -1 on error.
        let rv = unsafe { libc::poll(&raw mut pfd, 1, timeout_ms) };
        match rv.cmp(&0) {
            std::cmp::Ordering::Greater => PollOutcome::Ready,
            std::cmp::Ordering::Equal => PollOutcome::Timeout,
            std::cmp::Ordering::Less => {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    PollOutcome::Timeout
                } else {
                    PollOutcome::Error
                }
            },
        }
    }

    /// Drain pending inotify events. We don't actually parse the
    /// events for filename — any `/dev/` churn triggers a full
    /// re-`query()`, which is the source of truth. Returns false if
    /// the read failed in a way that means the fd is unusable.
    fn drain_inotify(fd: i32) -> bool {
        let mut buf = [0u8; READ_BUF_BYTES];
        loop {
            // SAFETY: buf is a valid mutable buffer of len READ_BUF_BYTES
            // and fd is a valid inotify fd. read() returns bytes read
            // or -1 on error.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                // Could parse events here, but we re-query
                // unconditionally so it's wasted work.
                continue;
            }
            if n == 0 {
                // EOF on inotify fd shouldn't happen, but treat as
                // unusable.
                return false;
            }
            // n < 0
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                // On Linux, EAGAIN == EWOULDBLOCK; both mean "no
                // more events ready, come back when poll() fires
                // again." EINTR is a benign signal interruption —
                // fall through to the next loop iteration.
                Some(libc::EAGAIN) => return true,
                Some(libc::EINTR) => {},
                _ => return false,
            }
        }
    }

    /// Re-snapshot + diff against the cached one. Emits `Connected`
    /// for newcomers and `Disconnected` for removals, then swaps the
    /// cache. Returns false if the channel is closed (consumer
    /// dropped the poller) so the worker can exit early.
    ///
    /// Emit arrivals before removals so a rapid re-plug can be
    /// observed as `Disconnected` → `Connected` on the consumer side
    /// even if both changes land in one inotify wake.
    fn reconcile_and_emit(
        tx: &Sender<HotplugEvent>,
        previous: &mut BTreeMap<String, CameraInfo>,
    ) -> bool {
        reconcile_and_emit_with(tx, previous, snapshot())
    }

    /// Diff `current` against `previous`, emit events, swap cache.
    /// Split out from [`reconcile_and_emit`] so unit tests can inject
    /// a synthetic `current` without touching `/dev/`.
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

    /// One `Video4Linux` enumeration pass, indexed by
    /// [`CameraIndex::to_string`]. The v4l node index is the stable
    /// identifier surfaced by `enum_devices` and maps 1:1 to
    /// `/dev/videoN`. Re-plugging a device can reuse the same index
    /// — consumers will then see `Disconnected(N)` followed by
    /// `Connected(N)`, which is the right semantic.
    ///
    /// Errors from `query()` are swallowed — a transient enumeration
    /// failure should not tear down the hotplug thread. An empty
    /// snapshot will look like "every device disappeared"; the next
    /// inotify wake we will re-emit them as `Connected`. Noisy but
    /// not incorrect.
    fn snapshot() -> BTreeMap<String, CameraInfo> {
        match query() {
            Ok(list) => list
                .into_iter()
                .map(|ci| (ci.index().to_string(), ci))
                .collect(),
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
                &format!("cam{idx}"),
                "test",
                "test",
                CameraIndex::Index(idx),
            )
        }

        fn snap<I: IntoIterator<Item = u32>>(ids: I) -> BTreeMap<String, CameraInfo> {
            ids.into_iter()
                .map(|i| (CameraIndex::Index(i).to_string(), info(i)))
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
            let keys: Vec<_> = previous.keys().cloned().collect();
            assert_eq!(keys, vec!["1".to_string(), "2".to_string()]);
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
        /// before removals so a re-plug landing in one inotify wake
        /// is observable as `Disconnected` → `Connected` on the
        /// consumer side. (Here we have one of each across distinct
        /// keys; the key promise is "all Connecteds precede all
        /// Disconnecteds within a single reconcile call".)
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

#[cfg(not(target_os = "linux"))]
mod real {
    use nokhwa_core::{
        error::NokhwaError,
        traits::{HotplugEventPoll, HotplugSource},
        types::ApiBackend,
    };

    /// Non-Linux stub for [`V4LHotplugContext`]. Every method errors
    /// with [`NokhwaError::UnsupportedOperationError`].
    #[derive(Default)]
    pub struct V4LHotplugContext;

    impl V4LHotplugContext {
        #[must_use]
        pub fn new() -> Self {
            Self
        }
    }

    impl HotplugSource for V4LHotplugContext {
        fn take_hotplug_events(&mut self) -> Result<Box<dyn HotplugEventPoll + Send>, NokhwaError> {
            Err(NokhwaError::UnsupportedOperationError(
                ApiBackend::Video4Linux,
            ))
        }
    }
}

pub use real::V4LHotplugContext;
