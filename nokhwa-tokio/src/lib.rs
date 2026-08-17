#![deny(clippy::pedantic)]
#![warn(clippy::all)]
#![allow(clippy::module_name_repetitions)]

//! Tokio integration for [`nokhwa`].
//!
//! Exposes [`TokioCameraRunner`], an async wrapper around the sync
//! [`nokhwa::CameraRunner`]. The wrapper owns the sync runner, spawns one
//! `spawn_blocking` forwarder per active channel, and exposes
//! `tokio::sync::mpsc::Receiver`s for `.recv().await`.
//!
//! # Drop semantics
//!
//! Dropping a [`TokioCameraRunner`] inside a tokio runtime returns
//! immediately; the underlying worker thread is joined on a
//! `spawn_blocking` task so the async executor is not blocked. Outside a
//! runtime, drop joins synchronously. For explicit shutdown, use
//! [`TokioCameraRunner::stop`]`.await`.
//!
//! On a **`current_thread`** runtime, the drop-queued `spawn_blocking`
//! task only runs after the next yield point. The drop itself still
//! returns immediately, but the physical sync worker thread keeps
//! running until the scheduler gets control back. If you need the worker
//! fully joined at a specific point, prefer `stop().await`.
//!
//! # Why `forwarders.abort()` is advisory
//!
//! Forwarder tasks are created via [`tokio::task::spawn_blocking`], which
//! runs on a dedicated blocking-thread pool. `spawn_blocking` tasks are
//! **not cancellable** — `JoinHandle::abort` marks them as aborted but
//! the OS thread keeps running until the current call (e.g.
//! `sync_rx.recv()`) returns. In practice that happens almost
//! immediately: [`TokioCameraRunner`] drops the sync `Receiver`s before
//! tearing down the inner runner, which causes the runner's senders to
//! disconnect, which unblocks the forwarder's `sync_rx.recv()` with
//! `Err`, and the forwarder exits. The `abort()` call is kept as a
//! belt-and-suspenders signal to the tokio scheduler.
//!
//! # Tokio features
//!
//! This crate depends on tokio with only `sync` and `rt` — the minimal set
//! needed for `mpsc` and `spawn_blocking` / `Handle::try_current`.

use std::fmt;

use nokhwa::{CameraRunner, OpenedCamera, RunnerConfig};
use nokhwa_core::buffer::Buffer;
use nokhwa_core::error::NokhwaError;
use nokhwa_core::traits::CameraEvent;
use nokhwa_core::types::{ControlValueSetter, KnownCameraControl};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Capacity of each forwarder's tokio-side channel.
///
/// This is separate from the sync-side [`RunnerConfig`] capacities: the
/// forwarder bridges a `std::sync::mpsc::Receiver` to a
/// `tokio::sync::mpsc::Sender`, and the tokio channel acts as a small
/// buffer to decouple the blocking-thread from the async consumer.
///
/// `32` is large enough to absorb brief async scheduling jitter without
/// starving the forwarder, small enough that back-pressure still reaches
/// the sync-side bounded channel. Not currently configurable; file an
/// issue if you need to tune it.
const FORWARDER_CAPACITY: usize = 32;

/// Async wrapper around [`CameraRunner`].
///
/// Build one with [`TokioCameraRunner::spawn`]; drain frames with
/// `frames_mut().map(|rx| rx.recv().await)` and friends.
pub struct TokioCameraRunner {
    frames: Option<mpsc::Receiver<Buffer>>,
    pictures: Option<mpsc::Receiver<Buffer>>,
    events: Option<mpsc::Receiver<CameraEvent>>,
    inner: Option<CameraRunner>,
    forwarders: Vec<JoinHandle<()>>,
}

impl fmt::Debug for TokioCameraRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokioCameraRunner")
            .field("frames", &self.frames.is_some())
            .field("pictures", &self.pictures.is_some())
            .field("events", &self.events.is_some())
            .field("inner", &self.inner)
            .field("forwarders", &self.forwarders.len())
            .finish()
    }
}

/// Build a tokio channel and spawn a blocking forwarder that bridges
/// items from a `std::sync::mpsc::Receiver` to the tokio side.
fn spawn_forwarder<T: Send + 'static>(
    sync_rx: std::sync::mpsc::Receiver<T>,
    forwarders: &mut Vec<JoinHandle<()>>,
) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel::<T>(FORWARDER_CAPACITY);
    forwarders.push(tokio::task::spawn_blocking(move || {
        while let Ok(item) = sync_rx.recv() {
            if tx.blocking_send(item).is_err() {
                break;
            }
        }
    }));
    rx
}

impl TokioCameraRunner {
    /// Build a sync [`CameraRunner`] and wire forwarder tasks for each
    /// available channel.
    ///
    /// # Errors
    /// Returns [`NokhwaError`] if the underlying [`CameraRunner::spawn`]
    /// fails.
    pub fn spawn(camera: OpenedCamera, cfg: RunnerConfig) -> Result<Self, NokhwaError> {
        let mut runner = CameraRunner::spawn(camera, cfg)?;
        let mut forwarders: Vec<JoinHandle<()>> = Vec::new();

        let frames = runner
            .take_frames()
            .map(|sync_rx| spawn_forwarder(sync_rx, &mut forwarders));
        let pictures = runner
            .take_pictures()
            .map(|sync_rx| spawn_forwarder(sync_rx, &mut forwarders));
        let events = runner
            .take_events()
            .map(|sync_rx| spawn_forwarder(sync_rx, &mut forwarders));

        Ok(Self {
            frames,
            pictures,
            events,
            inner: Some(runner),
            forwarders,
        })
    }

    /// Mutable access to the frames receiver. Call `recv().await` on it.
    pub fn frames_mut(&mut self) -> Option<&mut mpsc::Receiver<Buffer>> {
        self.frames.as_mut()
    }

    /// Mutable access to the pictures receiver.
    pub fn pictures_mut(&mut self) -> Option<&mut mpsc::Receiver<Buffer>> {
        self.pictures.as_mut()
    }

    /// Mutable access to the events receiver.
    pub fn events_mut(&mut self) -> Option<&mut mpsc::Receiver<CameraEvent>> {
        self.events.as_mut()
    }

    /// Trigger a shutter capture on the underlying sync runner.
    ///
    /// # Errors
    /// See [`CameraRunner::trigger`].
    pub fn trigger(&self) -> Result<(), NokhwaError> {
        self.inner
            .as_ref()
            .ok_or_else(|| NokhwaError::general("runner already stopped"))?
            .trigger()
    }

    /// Set a camera control on the underlying sync runner.
    ///
    /// # Errors
    /// See [`CameraRunner::set_control`].
    pub fn set_control(
        &self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.inner
            .as_ref()
            .ok_or_else(|| NokhwaError::general("runner already stopped"))?
            .set_control(id, value)
    }

    /// Stop forwarders and join the underlying sync runner on a
    /// `spawn_blocking` task.
    ///
    /// `stop()` tears the runner down immediately and may drop frames or
    /// pictures that are still in flight: the async receivers are dropped
    /// before each forwarder's pending `blocking_send` completes, so an
    /// item a forwarder has pulled from the sync side but not yet handed to
    /// the async side is discarded. This is intended shutdown behaviour for
    /// a live stream; if you need every queued item, drain the receivers
    /// with `recv().await` until they return `None` before calling `stop()`.
    ///
    /// # Errors
    /// Returns [`NokhwaError`] only if the `spawn_blocking` task panics.
    pub async fn stop(mut self) -> Result<(), NokhwaError> {
        // `abort()` on spawn_blocking tasks is advisory — see the
        // crate-level note. Forwarders actually exit because the async
        // receivers are dropped below: once `self.frames/pictures/events`
        // are set to `None`, the next `blocking_send` inside each
        // forwarder returns `Err` and the loop exits. The `abort()` call
        // is belt-and-suspenders to signal the tokio scheduler; the actual
        // exit mechanism is the broken async sender, not `sync_rx.recv()`.
        for f in self.forwarders.drain(..) {
            f.abort();
        }
        // Drop the async receivers so forwarders observe the closed tx
        // and exit cleanly (their blocking_send returns an error).
        self.frames = None;
        self.pictures = None;
        self.events = None;
        if let Some(inner) = self.inner.take() {
            tokio::task::spawn_blocking(move || drop(inner))
                .await
                .map_err(|e| NokhwaError::general(format!("runner join failed: {e}")))?;
        }
        Ok(())
    }
}

impl Drop for TokioCameraRunner {
    fn drop(&mut self) {
        // See note on `stop()`: `abort()` is advisory for spawn_blocking.
        for f in self.forwarders.drain(..) {
            f.abort();
        }
        self.frames = None;
        self.pictures = None;
        self.events = None;
        if let Some(inner) = self.inner.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn_blocking(move || drop(inner));
                },
                Err(_) => {
                    // No tokio runtime: the calling thread is not an
                    // executor task, so a synchronous join is safe.
                    drop(inner);
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{spawn_forwarder, FORWARDER_CAPACITY};
    use tokio::task::JoinHandle;

    /// `FORWARDER_CAPACITY` is the documented buffer size for the
    /// tokio-side mpsc channel that bridges blocking-thread sends
    /// to async receivers. The crate-level docs note `32` as a
    /// considered choice ("large enough to absorb brief async
    /// scheduling jitter without starving the forwarder, small
    /// enough that back-pressure still reaches the sync-side
    /// bounded channel"). Pin the exact value so a future tweak
    /// is a deliberate, reviewed change rather than a silent
    /// memory-vs-throughput shift.
    #[test]
    fn forwarder_capacity_is_thirty_two() {
        assert_eq!(FORWARDER_CAPACITY, 32);
    }

    /// `spawn_forwarder` bridges items from a `std::sync::mpsc::Receiver`
    /// to a `tokio::sync::mpsc::Receiver`. Pin the contract: items
    /// produced on the sync side reach the async side in order.
    #[tokio::test(flavor = "current_thread")]
    async fn forwarder_bridges_sync_to_async_in_order() {
        let (sync_tx, sync_rx) = std::sync::mpsc::channel::<u32>();
        let mut forwarders: Vec<JoinHandle<()>> = Vec::new();
        let mut async_rx = spawn_forwarder(sync_rx, &mut forwarders);
        assert_eq!(
            forwarders.len(),
            1,
            "spawn_forwarder must register one forwarder JoinHandle"
        );

        for i in 0..10u32 {
            sync_tx.send(i).expect("sync_tx.send");
        }
        drop(sync_tx);

        let mut received = Vec::new();
        while let Some(v) = async_rx.recv().await {
            received.push(v);
        }
        assert_eq!(
            received,
            (0..10).collect::<Vec<_>>(),
            "forwarder must preserve order across the sync→async bridge"
        );
    }

    /// When the sync producer disconnects, the async receiver must
    /// observe `None` (channel closed) and the forwarder task must
    /// exit cleanly. Catches a regression where the forwarder
    /// accidentally swallows the `Err` branch from `sync_rx.recv()`
    /// and stays alive holding the tokio sender, which would prevent
    /// the async side from ever observing closure.
    #[tokio::test(flavor = "current_thread")]
    async fn forwarder_closes_async_when_sync_producer_drops() {
        let (sync_tx, sync_rx) = std::sync::mpsc::channel::<u32>();
        let mut forwarders: Vec<JoinHandle<()>> = Vec::new();
        let mut async_rx = spawn_forwarder(sync_rx, &mut forwarders);
        drop(sync_tx);
        // After the sync sender is dropped, the forwarder's
        // sync_rx.recv() returns Err and the task exits, dropping
        // its tokio sender; the async recv() then yields None.
        assert!(
            async_rx.recv().await.is_none(),
            "async receiver must observe closure when sync producer disconnects"
        );
    }

    /// `spawn_forwarder` is called once per channel inside
    /// `TokioCameraRunner::spawn` — frames, pictures, and events each
    /// get an independent forwarder appended to a shared `Vec`. Pin
    /// that the helper *appends* (does not replace) and that each
    /// forwarder routes to its own receiver so streams stay isolated.
    #[tokio::test(flavor = "current_thread")]
    async fn forwarder_appends_independent_handles_for_each_call() {
        let (a_tx, a_rx) = std::sync::mpsc::channel::<u32>();
        let (b_tx, b_rx) = std::sync::mpsc::channel::<u32>();
        let mut forwarders: Vec<JoinHandle<()>> = Vec::new();
        let mut async_a = spawn_forwarder(a_rx, &mut forwarders);
        let mut async_b = spawn_forwarder(b_rx, &mut forwarders);
        assert_eq!(
            forwarders.len(),
            2,
            "each spawn_forwarder call must push exactly one JoinHandle"
        );

        a_tx.send(1).expect("a_tx.send");
        b_tx.send(99).expect("b_tx.send");
        drop(a_tx);
        drop(b_tx);

        let mut got_a = Vec::new();
        while let Some(v) = async_a.recv().await {
            got_a.push(v);
        }
        let mut got_b = Vec::new();
        while let Some(v) = async_b.recv().await {
            got_b.push(v);
        }
        assert_eq!(got_a, vec![1], "stream A leaked items from stream B");
        assert_eq!(got_b, vec![99], "stream B leaked items from stream A");
    }

    /// Empty-then-disconnect: a forwarder spun up for a channel that
    /// never produces anything must still surface closure to the
    /// async side. Distinct from the producer-drops test because no
    /// item ever traverses the bridge — guards against a regression
    /// where the forwarder waits for at least one successful send
    /// before honouring closure.
    #[tokio::test(flavor = "current_thread")]
    async fn forwarder_surfaces_closure_with_no_items_sent() {
        let (sync_tx, sync_rx) = std::sync::mpsc::channel::<u32>();
        let mut forwarders: Vec<JoinHandle<()>> = Vec::new();
        let mut async_rx = spawn_forwarder(sync_rx, &mut forwarders);
        drop(sync_tx);
        assert!(
            async_rx.recv().await.is_none(),
            "empty channel must still close the async side"
        );
    }

    /// The forwarder bridges any `Send + 'static` payload; pin that the
    /// generic parameter actually carries non-`Copy` owned data
    /// end-to-end without losing values. A future refactor that
    /// accidentally narrows the bound (e.g. to `Copy`) would break
    /// the `Buffer` channel — this test fails fast on that mistake.
    #[tokio::test(flavor = "current_thread")]
    async fn forwarder_carries_owned_non_copy_payloads() {
        let (sync_tx, sync_rx) = std::sync::mpsc::channel::<String>();
        let mut forwarders: Vec<JoinHandle<()>> = Vec::new();
        let mut async_rx = spawn_forwarder(sync_rx, &mut forwarders);

        sync_tx.send("hello".to_string()).expect("sync_tx.send");
        sync_tx.send("world".to_string()).expect("sync_tx.send");
        drop(sync_tx);

        let mut received = Vec::new();
        while let Some(v) = async_rx.recv().await {
            received.push(v);
        }
        assert_eq!(received, vec!["hello".to_string(), "world".to_string()]);
    }
}
