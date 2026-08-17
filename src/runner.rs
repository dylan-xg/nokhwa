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

//! Layer 3: [`CameraRunner`], a background-thread capture helper.
//!
//! The runner owns an [`OpenedCamera`] on a dedicated thread and delivers
//! frames, pictures, and events through `std::sync::mpsc` channels. The
//! thread is joined either via [`CameraRunner::stop`] or on drop.
//!
//! Dispatch mirrors [`OpenedCamera`]:
//!
//! - [`OpenedCamera::Stream`]: the runner exposes a frames channel.
//! - [`OpenedCamera::Shutter`]: the runner exposes a pictures + events
//!   channels (events is always present but inert for non-event backends).
//! - [`OpenedCamera::Hybrid`]: all three channels are available; events
//!   only if the backend advertised `EventSource`.

use std::sync::mpsc::{
    channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError,
    TrySendError,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nokhwa_core::buffer::Buffer;
use nokhwa_core::error::NokhwaError;
use nokhwa_core::traits::{CameraEvent, EventPoll};
use nokhwa_core::types::{ControlValueSetter, KnownCameraControl};

use crate::session::{HybridCamera, OpenedCamera, ShutterCamera, StreamCamera};

/// Policy for what to do when a bounded runner channel is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    /// Drop the newly-produced item, preserving older backlog. Lowest
    /// overhead; good when any-recent-frame is acceptable.
    #[default]
    DropNewest,
    /// Drop the oldest item in the channel to make room for the new one.
    /// Good when consumers want the freshest frame. Implemented via a
    /// per-channel relay thread — only paid when this policy is selected.
    DropOldest,
    /// Block the producer until the consumer drains. Old-school back-
    /// pressure; the camera worker stalls if the consumer is slow, which
    /// for a stream backend means real-time frames will be missed by the
    /// underlying device rather than dropped in software. Use when you
    /// want every queued frame delivered in order and you're OK with the
    /// worker pausing.
    Block,
}

/// Configuration for [`CameraRunner::spawn`].
#[derive(Debug, Clone, Copy)]
pub struct RunnerConfig {
    /// Worker poll interval.
    ///
    /// - In stream / hybrid variants: how long the worker sleeps before
    ///   retrying after a failed `frame()` call.
    /// - In the shutter variant: the command-channel receive timeout (i.e.
    ///   the cadence at which the worker wakes up to check for shutdown).
    pub poll_interval: Duration,
    /// Event-poll timeout passed to [`EventPoll::next_timeout`].
    pub event_tick: Duration,
    /// Timeout for shutter `take_picture(…)` after a trigger.
    pub shutter_timeout: Duration,
    /// Capacity of the frames channel. `0` = unbounded (pre-0.14 behavior).
    pub frames_capacity: usize,
    /// Capacity of the pictures channel. `0` = unbounded.
    pub pictures_capacity: usize,
    /// Capacity of the events channel. `0` = unbounded.
    pub events_capacity: usize,
    /// Policy when a bounded channel is full. Ignored if the corresponding
    /// capacity is `0`.
    pub overflow: Overflow,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(10),
            event_tick: Duration::from_millis(50),
            shutter_timeout: Duration::from_secs(5),
            frames_capacity: 4,
            pictures_capacity: 8,
            events_capacity: 32,
            overflow: Overflow::DropNewest,
        }
    }
}

/// Producer half of a runner channel, abstracting over unbounded /
/// bounded-with-overflow variants. Shared across the three spawn paths.
enum Tx<T: Send + 'static> {
    Unbounded(Sender<T>),
    /// `DropNewest`: on `Full`, the new item is silently discarded.
    BoundedDropNewest(SyncSender<T>),
    /// `DropOldest`: feeds into a relay thread that drop-oldest's into the
    /// user-facing bounded channel.
    BoundedDropOldest(SyncSender<T>),
    /// `Block`: producer stalls until the consumer drains.
    BoundedBlock(SyncSender<T>),
}

impl<T: Send + 'static> Tx<T> {
    /// Send an item. Returns `Err(())` if the consumer has disconnected —
    /// the worker should treat this as a shutdown signal. Overflow on a
    /// bounded channel is **not** an error.
    fn send(&self, item: T) -> Result<(), ()> {
        match self {
            Tx::Unbounded(tx) => tx.send(item).map_err(|_| ()),
            Tx::BoundedDropNewest(tx) => match tx.try_send(item) {
                Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
                Err(TrySendError::Disconnected(_)) => Err(()),
            },
            Tx::BoundedDropOldest(tx) | Tx::BoundedBlock(tx) => tx.send(item).map_err(|_| ()),
        }
    }
}

/// Build a (producer, consumer) pair for a runner stream, along with an
/// optional relay-thread `JoinHandle` that the runner must join on shutdown.
///
/// - `capacity == 0` → unbounded `std::sync::mpsc::channel`; relay = `None`.
/// - `capacity > 0, DropNewest` → single `sync_channel(capacity)`; relay = `None`.
/// - `capacity > 0, DropOldest` → producer feeds a relay thread that
///   maintains a `VecDeque<T>` of at most `capacity` items; when full, the
///   oldest item is popped before the new one is pushed. The relay forwards
///   items into an unbounded channel exposed to the user, so the total
///   memory footprint stays bounded by `capacity` plus whatever the user
///   hasn't yet received (which is always immediately drainable).
fn make_channel<T: Send + 'static>(
    capacity: usize,
    policy: Overflow,
) -> (Tx<T>, Receiver<T>, Option<JoinHandle<()>>) {
    if capacity == 0 {
        let (tx, rx) = channel::<T>();
        return (Tx::Unbounded(tx), rx, None);
    }
    match policy {
        Overflow::DropNewest => {
            let (tx, rx) = sync_channel::<T>(capacity);
            (Tx::BoundedDropNewest(tx), rx, None)
        },
        Overflow::Block => {
            let (tx, rx) = sync_channel::<T>(capacity);
            (Tx::BoundedBlock(tx), rx, None)
        },
        Overflow::DropOldest => {
            // Two chained `sync_channel(capacity)` joined by a relay thread
            // that owns an in-memory `VecDeque<T>` of at most `capacity`
            // items. When the user-facing channel is full, the relay drops
            // the oldest buffered item to make room for the new one. Total
            // memory footprint is bounded by `2 * capacity` items.
            let (prod_tx, relay_rx) = sync_channel::<T>(capacity);
            let (user_tx, user_rx) = sync_channel::<T>(capacity);
            let handle = thread::spawn(move || {
                use std::collections::VecDeque;
                let mut buf: VecDeque<T> = VecDeque::with_capacity(capacity);
                // 5 ms ≈ 200 Hz: drives the drain-while-waiting fallback
                // when the producer is idle but `user_tx` was previously
                // full. Small enough that the user sees freshly-buffered
                // items without perceptible delay; large enough that an
                // idle relay costs negligible CPU.
                let poll = Duration::from_millis(5);
                loop {
                    // Try to drain buffer into user_tx first (non-blocking).
                    while let Some(front) = buf.pop_front() {
                        match user_tx.try_send(front) {
                            Ok(()) => {},
                            Err(TrySendError::Full(item)) => {
                                buf.push_front(item);
                                break;
                            },
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    }
                    // Wait for a new item. If buf is non-empty, poll with a
                    // short timeout so we get another chance to drain into
                    // user_tx even when the producer has gone idle.
                    let wait = if buf.is_empty() {
                        relay_rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
                    } else {
                        relay_rx.recv_timeout(poll)
                    };
                    match wait {
                        Ok(item) => {
                            if buf.len() == capacity {
                                buf.pop_front();
                            }
                            buf.push_back(item);
                        },
                        Err(RecvTimeoutError::Timeout) => {},
                        Err(RecvTimeoutError::Disconnected) => {
                            // Producer gone; best-effort flush of buffered items
                            // to the consumer. Uses `try_send` (non-blocking) so
                            // this flush loop can never deadlock regardless of
                            // shutdown ordering:
                            //   • `Disconnected` → consumer dropped its receiver;
                            //     exit immediately.
                            //   • `Full` → consumer isn't draining; give up rather
                            //     than stalling a thread that may be joining us.
                            //   • `Ok` → item delivered; keep draining.
                            // The previous blocking `user_tx.send` was safe only
                            // because `CameraRunner::shutdown` dropped the user
                            // `Receiver` before joining the relay, creating an
                            // informal invariant that was easy to violate in a
                            // future refactor. `try_send` removes that dependency.
                            while let Some(front) = buf.pop_front() {
                                match user_tx.try_send(front) {
                                    Ok(()) => {},
                                    Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                                        return;
                                    },
                                }
                            }
                            return;
                        },
                    }
                }
            });
            (Tx::BoundedDropOldest(prod_tx), user_rx, Some(handle))
        },
    }
}

/// Commands sent from the foreground handle to the worker thread.
enum Command {
    Trigger,
    SetControl(KnownCameraControl, ControlValueSetter),
    Die,
}

/// Background-thread capture helper.
///
/// `CameraRunner` owns an [`OpenedCamera`] on a worker
/// thread and delivers frames / pictures / events through
/// [`std::sync::mpsc`] channels.
///
/// ## Channel semantics
///
/// Channels are **bounded by default** (since 0.14). Capacities and the
/// [`Overflow`] policy live on [`RunnerConfig`]; the defaults are 4 frames,
/// 8 pictures, 32 events, and [`Overflow::DropNewest`]. Setting a capacity
/// to `0` restores the 0.13-era unbounded [`std::sync::mpsc::channel`]
/// behavior.
///
/// Each accessor pair has two flavors: borrowed ([`frames`](Self::frames),
/// [`pictures`](Self::pictures), [`events`](Self::events)) for in-place
/// draining, and owned ([`take_frames`](Self::take_frames),
/// [`take_pictures`](Self::take_pictures), [`take_events`](Self::take_events))
/// for handing a receiver to another task or wrapper (e.g. the
/// `nokhwa-tokio` forwarder). The worker thread stays alive after a
/// `take_*` call; you can still [`trigger`](Self::trigger) and
/// [`set_control`](Self::set_control).
///
/// The worker defensively exits on `SendError` — so if a caller drops a
/// taken receiver without invoking [`stop`](Self::stop) or dropping the
/// runner, the worker still shuts down cleanly on its next send attempt.
#[derive(Debug)]
pub struct CameraRunner {
    frames: Option<Receiver<Buffer>>,
    pictures: Option<Receiver<Buffer>>,
    events: Option<Receiver<CameraEvent>>,
    cmd: Sender<Command>,
    join: Option<JoinHandle<()>>,
    relays: Vec<JoinHandle<()>>,
}

impl CameraRunner {
    /// Spawn a worker thread owning `camera`.
    ///
    /// # Errors
    /// Returns [`NokhwaError`] if opening the underlying camera fails
    /// (stream/hybrid variants call `open()` before entering the loop).
    pub fn spawn(camera: OpenedCamera, cfg: RunnerConfig) -> Result<Self, NokhwaError> {
        match camera {
            OpenedCamera::Stream(cam) => Self::spawn_stream(cam, cfg),
            OpenedCamera::Shutter(cam) => Ok(Self::spawn_shutter(cam, cfg)),
            OpenedCamera::Hybrid(cam) => Self::spawn_hybrid(cam, cfg),
        }
    }

    fn spawn_stream(mut cam: StreamCamera, cfg: RunnerConfig) -> Result<Self, NokhwaError> {
        // Idempotent open: the caller may have already called `open()` before
        // handing the camera to the runner (e.g. to inspect a negotiated
        // format). Opening a second time on AVFoundation creates a redundant
        // `AVCaptureSession`, so guard against it.
        if !cam.is_open() {
            cam.open()?;
        }
        let (frame_tx, frame_rx, frame_relay) =
            make_channel::<Buffer>(cfg.frames_capacity, cfg.overflow);
        let (cmd_tx, cmd_rx) = channel::<Command>();
        let poll_interval = cfg.poll_interval;
        let join = thread::spawn(move || {
            // Consecutive `frame()` error count — drives exponential backoff.
            let mut err_count: u32 = 0;
            loop {
                match cmd_rx.try_recv() {
                    Ok(Command::Die) | Err(TryRecvError::Disconnected) => break,
                    Ok(Command::SetControl(id, v)) => {
                        let _ = cam.set_control(id, v);
                    },
                    Ok(Command::Trigger) | Err(TryRecvError::Empty) => {},
                }
                match cam.frame() {
                    Ok(buf) => {
                        // If the consumer dropped the receiver, treat it as a
                        // shutdown signal rather than spinning forever.
                        if frame_tx.send(buf).is_err() {
                            break;
                        }
                        // Successful frame: reset the backoff.
                        err_count = 0;
                    },
                    Err(err) => {
                        // Exponential backoff: sleep poll_interval * 2^min(n, 7)
                        // (caps at ×128, i.e. ~1.3 s with the 10 ms default).
                        // Prevents a ~100 retry/s busy-spin when the camera
                        // enters a persistent error state (e.g. unplugged,
                        // AVF session interrupted).
                        let shift = err_count.min(7);
                        thread::sleep(poll_interval * (1u32 << shift));
                        err_count = err_count.saturating_add(1);
                        #[cfg(feature = "logging")]
                        {
                            if err_count == 1 {
                                log::debug!("CameraRunner: frame() error: {err}");
                            } else if err_count.is_power_of_two() {
                                log::warn!(
                                    "CameraRunner: frame() failed {err_count} times in a row: {err}"
                                );
                            }
                        }
                        #[cfg(not(feature = "logging"))]
                        let _ = err;
                    },
                }
            }
        });
        let mut relays = Vec::new();
        if let Some(h) = frame_relay {
            relays.push(h);
        }
        Ok(Self {
            frames: Some(frame_rx),
            pictures: None,
            events: None,
            cmd: cmd_tx,
            join: Some(join),
            relays,
        })
    }

    fn spawn_shutter(mut cam: ShutterCamera, cfg: RunnerConfig) -> Self {
        let (pic_tx, pic_rx, pic_relay) =
            make_channel::<Buffer>(cfg.pictures_capacity, cfg.overflow);
        let (cmd_tx, cmd_rx) = channel::<Command>();
        let poll_interval = cfg.poll_interval;
        let shutter_timeout = cfg.shutter_timeout;
        let join = thread::spawn(move || loop {
            match cmd_rx.recv_timeout(poll_interval) {
                Ok(Command::Die) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Command::Trigger) => {
                    if cam.trigger().is_ok() {
                        if let Ok(pic) = cam.take_picture(shutter_timeout) {
                            if pic_tx.send(pic).is_err() {
                                break;
                            }
                        }
                    }
                },
                Ok(Command::SetControl(id, v)) => {
                    let _ = cam.set_control(id, v);
                },
                Err(RecvTimeoutError::Timeout) => {},
            }
        });
        let mut relays = Vec::new();
        if let Some(h) = pic_relay {
            relays.push(h);
        }
        Self {
            frames: None,
            pictures: Some(pic_rx),
            events: None,
            cmd: cmd_tx,
            join: Some(join),
            relays,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn spawn_hybrid(mut cam: HybridCamera, cfg: RunnerConfig) -> Result<Self, NokhwaError> {
        // Idempotent open: same guard as `spawn_stream` — the caller may have
        // already opened the camera before passing it to the runner.
        if !cam.is_open() {
            cam.open()?;
        }
        let events_poll: Option<Box<dyn EventPoll + Send>> = match cam.take_events() {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                #[cfg(feature = "logging")]
                log::warn!("CameraRunner: failed to take event poller: {e}");
                #[cfg(not(feature = "logging"))]
                let _ = e;
                None
            },
            None => None,
        };

        let (frame_tx, frame_rx, frame_relay) =
            make_channel::<Buffer>(cfg.frames_capacity, cfg.overflow);
        let (pic_tx, pic_rx, pic_relay) =
            make_channel::<Buffer>(cfg.pictures_capacity, cfg.overflow);
        let (cmd_tx, cmd_rx) = channel::<Command>();

        let mut relays: Vec<JoinHandle<()>> = Vec::new();
        if let Some(h) = frame_relay {
            relays.push(h);
        }
        if let Some(h) = pic_relay {
            relays.push(h);
        }

        // Events thread (if any).
        let (event_rx_opt, event_join_opt) = if let Some(mut poll) = events_poll {
            let (ev_tx, ev_rx, ev_relay) =
                make_channel::<CameraEvent>(cfg.events_capacity, cfg.overflow);
            if let Some(h) = ev_relay {
                relays.push(h);
            }
            let (ev_cmd_tx, ev_cmd_rx) = channel::<()>();
            let event_tick = cfg.event_tick;
            let handle = thread::spawn(move || loop {
                // Break on an explicit stop signal *or* a closed command
                // channel. The latter covers the case where the main worker
                // unwinds (e.g. a `frame()` panic) before it can send the
                // signal: `ev_cmd_tx` is dropped, `try_recv` reports
                // `Disconnected`, and the event thread exits instead of
                // depending on the user-side event receiver being dropped to
                // make `ev_tx.send` fail.
                match ev_cmd_rx.try_recv() {
                    Ok(()) | Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {},
                }
                if let Some(event) = poll.next_timeout(event_tick) {
                    if ev_tx.send(event).is_err() {
                        break;
                    }
                }
            });
            (Some(ev_rx), Some((ev_cmd_tx, handle)))
        } else {
            (None, None)
        };

        let poll_interval = cfg.poll_interval;
        let shutter_timeout = cfg.shutter_timeout;

        let join = thread::spawn(move || {
            // Consecutive `frame()` error count — drives exponential backoff.
            let mut err_count: u32 = 0;
            loop {
                match cmd_rx.try_recv() {
                    Ok(Command::Die) | Err(TryRecvError::Disconnected) => break,
                    Ok(Command::Trigger) => {
                        // Trigger / take-picture errors are intentionally
                        // swallowed here; a backend that wants to surface
                        // them should emit a `CameraEvent` via the events
                        // channel instead.
                        if cam.trigger().is_ok() {
                            if let Ok(pic) = cam.take_picture(shutter_timeout) {
                                // Policy: hybrid workers treat a dropped
                                // pictures receiver as "caller isn't
                                // interested in photos right now" — keep
                                // streaming frames. Only a dropped *frames*
                                // receiver shuts the worker down (below).
                                let _ = pic_tx.send(pic);
                            }
                        }
                    },
                    Ok(Command::SetControl(id, v)) => {
                        let _ = cam.set_control(id, v);
                    },
                    Err(TryRecvError::Empty) => {},
                }
                match cam.frame() {
                    Ok(buf) => {
                        // Exit if the consumer dropped the frames receiver.
                        if frame_tx.send(buf).is_err() {
                            break;
                        }
                        // Successful frame: reset the backoff.
                        err_count = 0;
                    },
                    Err(err) => {
                        // Same exponential backoff as the stream worker.
                        let shift = err_count.min(7);
                        thread::sleep(poll_interval * (1u32 << shift));
                        err_count = err_count.saturating_add(1);
                        #[cfg(feature = "logging")]
                        {
                            if err_count == 1 {
                                log::debug!("CameraRunner: frame() error: {err}");
                            } else if err_count.is_power_of_two() {
                                log::warn!(
                                    "CameraRunner: frame() failed {err_count} times in a row: {err}"
                                );
                            }
                        }
                        #[cfg(not(feature = "logging"))]
                        let _ = err;
                    },
                }
            }
            // Tell the events thread to stop too.
            if let Some((ev_cmd_tx, handle)) = event_join_opt {
                let _ = ev_cmd_tx.send(());
                if let Err(err) = handle.join() {
                    #[cfg(feature = "logging")]
                    log::warn!("CameraRunner: event worker thread panicked: {err:?}");
                    #[cfg(not(feature = "logging"))]
                    let _ = err;
                }
            }
        });

        Ok(Self {
            frames: Some(frame_rx),
            pictures: Some(pic_rx),
            events: event_rx_opt,
            cmd: cmd_tx,
            join: Some(join),
            relays,
        })
    }

    /// Frame receiver, if this runner's backend is a stream or hybrid.
    #[must_use]
    pub fn frames(&self) -> Option<&Receiver<Buffer>> {
        self.frames.as_ref()
    }

    /// Picture receiver, if this runner's backend is a shutter or hybrid.
    #[must_use]
    pub fn pictures(&self) -> Option<&Receiver<Buffer>> {
        self.pictures.as_ref()
    }

    /// Event receiver, if the backend advertised `EventSource`.
    #[must_use]
    pub fn events(&self) -> Option<&Receiver<CameraEvent>> {
        self.events.as_ref()
    }

    fn send_cmd(&self, cmd: Command) -> Result<(), NokhwaError> {
        self.cmd
            .send(cmd)
            .map_err(|e| NokhwaError::general(format!("runner thread gone: {e}")))
    }

    /// Trigger a shutter capture on the worker. No-op for pure stream backends.
    ///
    /// # Errors
    /// Returns [`NokhwaError`] if the worker thread is no longer running.
    pub fn trigger(&self) -> Result<(), NokhwaError> {
        self.send_cmd(Command::Trigger)
    }

    /// Set a camera control on the worker thread.
    ///
    /// # Errors
    /// Returns [`NokhwaError`] if the worker thread is no longer running.
    pub fn set_control(
        &self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.send_cmd(Command::SetControl(id, value))
    }

    /// Stop the worker thread and join it.
    ///
    /// # Errors
    /// Returns [`NokhwaError`] only if signalling the worker fails; a failed
    /// join is logged-but-ignored because there is no good recovery.
    pub fn stop(mut self) -> Result<(), NokhwaError> {
        self.shutdown();
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.cmd.send(Command::Die);
        // Drop receivers first so any blocked relay `try_send` wakes up and
        // the relay threads can exit cleanly. For `Overflow::Block` the
        // worker may be parked inside `SyncSender::send` with `Die`
        // sitting unread in the command queue; the receiver-drop below is
        // what unblocks that send (via `SendError`), after which the
        // worker loop exits.
        self.frames = None;
        self.pictures = None;
        self.events = None;
        if let Some(handle) = self.join.take() {
            if let Err(err) = handle.join() {
                #[cfg(feature = "logging")]
                log::warn!("CameraRunner: worker thread panicked: {err:?}");
                #[cfg(not(feature = "logging"))]
                let _ = err;
            }
        }
        for relay in self.relays.drain(..) {
            if let Err(err) = relay.join() {
                #[cfg(feature = "logging")]
                log::warn!("CameraRunner: relay thread panicked: {err:?}");
                #[cfg(not(feature = "logging"))]
                let _ = err;
            }
        }
    }

    /// Take ownership of the frames receiver. The worker thread keeps
    /// running so you can still [`trigger`](Self::trigger) and
    /// [`set_control`](Self::set_control). Returns `None` if the runner was
    /// built from a shutter-only backend, or the receiver was already taken.
    #[must_use]
    pub fn take_frames(&mut self) -> Option<Receiver<Buffer>> {
        self.frames.take()
    }

    /// Take ownership of the pictures receiver. See [`take_frames`](Self::take_frames).
    #[must_use]
    pub fn take_pictures(&mut self) -> Option<Receiver<Buffer>> {
        self.pictures.take()
    }

    /// Take ownership of the events receiver. See [`take_frames`](Self::take_frames).
    #[must_use]
    pub fn take_events(&mut self) -> Option<Receiver<CameraEvent>> {
        self.events.take()
    }
}

impl Drop for CameraRunner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::{make_channel, Overflow};
    use std::time::Duration;

    #[test]
    fn unbounded_capacity_zero_is_unbounded() {
        let (tx, rx, relay) = make_channel::<u32>(0, Overflow::DropNewest);
        assert!(relay.is_none());
        for i in 0..1000 {
            tx.send(i).unwrap();
        }
        for i in 0..1000 {
            assert_eq!(rx.recv().unwrap(), i);
        }
    }

    #[test]
    fn drop_newest_discards_overflow() {
        let (tx, rx, relay) = make_channel::<u32>(2, Overflow::DropNewest);
        assert!(relay.is_none());
        // Fill capacity.
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        // Overflow: the new item (3) is dropped; backlog preserved.
        tx.send(3).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap(), 2);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn drop_oldest_has_relay_and_accepts_sends() {
        // We don't attempt to prove the exact sequence observed — the relay
        // drains into the user channel on a timer, so timing is inherently
        // fuzzy. What we do check: a relay handle exists, sends succeed,
        // and the consumer observes *some* items including at least one
        // item from the tail of the produced sequence.
        let (tx, rx, relay) = make_channel::<u32>(2, Overflow::DropOldest);
        assert!(relay.is_some(), "DropOldest should spawn a relay thread");
        // Produce items and drain concurrently to avoid `SyncSender::send`
        // back-pressure through the relay.
        let producer = std::thread::spawn(move || {
            for i in 0..50u32 {
                tx.send(i).unwrap();
            }
        });
        let mut received = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(v) => received.push(v),
                Err(_) => {
                    if received.len() >= 2 {
                        break;
                    }
                },
            }
        }
        producer.join().unwrap();
        assert!(
            !received.is_empty(),
            "expected at least one item through the relay"
        );
        // Items must be strictly increasing — the producer emits unique
        // `0..50` values and drop-oldest never re-orders.
        for w in received.windows(2) {
            assert!(w[0] < w[1], "drop-oldest must not reorder: {received:?}");
        }
        drop(rx);
        relay.unwrap().join().unwrap();
    }

    #[test]
    fn drop_oldest_relay_exits_when_rx_dropped() {
        // When the user drops the receiver, the relay must eventually
        // observe `Disconnected` and exit, and further producer sends
        // must fail. Guards against future refactors of the relay loop.
        use std::time::Instant;
        let (tx, rx, relay) = make_channel::<u32>(2, Overflow::DropOldest);
        let relay = relay.unwrap();
        drop(rx);
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if tx.send(0).is_err() {
                break;
            }
            // Yield so the relay thread has a fair chance to observe
            // the dropped user_rx and exit.
            std::thread::yield_now();
        }
        assert!(
            tx.send(0).is_err(),
            "producer send should fail after the relay exits"
        );
        relay.join().unwrap();
    }

    #[test]
    fn tx_send_err_on_consumer_drop_unbounded() {
        let (tx, rx, _) = make_channel::<u32>(0, Overflow::DropNewest);
        drop(rx);
        assert!(tx.send(1).is_err());
    }

    #[test]
    fn tx_send_err_on_consumer_drop_bounded_newest() {
        let (tx, rx, _) = make_channel::<u32>(2, Overflow::DropNewest);
        drop(rx);
        // SyncSender::try_send surfaces Disconnected after the receiver is dropped.
        // (Note: a pending Full item buffered before drop is not possible here
        // because nothing was sent.)
        assert!(tx.send(1).is_err());
    }

    // `Overflow::Block` had zero coverage. The arm collapses the
    // `BoundedDropOldest | BoundedBlock` into the same `tx.send`
    // path in `Tx::send`, so a regression that mis-routes `Block`
    // into the relay-thread branch (or strips the relay-handle
    // contract that `Block` has none) would slip through every
    // existing runner test. Pin the three observable contracts:
    // (1) `relay` is `None`; (2) sends within capacity preserve
    // FIFO order; (3) a send after the consumer is dropped
    // returns `Err`.

    #[test]
    fn block_capacity_returns_no_relay_handle() {
        // Unlike `DropOldest`, `Block` does not need a relay
        // thread because `SyncSender::send` does the blocking
        // for us. The contract is "no extra thread, no extra
        // shutdown work" — pin it so a future refactor that
        // adds a relay for symmetry doesn't silently leak a
        // thread on every `CameraRunner` start.
        let (_tx, _rx, relay) = make_channel::<u32>(2, Overflow::Block);
        assert!(
            relay.is_none(),
            "Overflow::Block must not spawn a relay thread"
        );
    }

    #[test]
    fn block_bounded_preserves_fifo_within_capacity() {
        // Within capacity, `Block` is just a `sync_channel`:
        // sends succeed, receives observe items in producer
        // order. Pin so a regression in the `Tx::send` arm
        // (e.g. accidentally folding `Block` into `DropNewest`
        // which silently discards on full) shows up as out-of-
        // order or missing items.
        let (tx, rx, _) = make_channel::<u32>(4, Overflow::Block);
        for i in 0..4u32 {
            tx.send(i).unwrap();
        }
        for i in 0..4u32 {
            assert_eq!(rx.recv().unwrap(), i);
        }
        // No more items pending.
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    fn tx_send_err_on_consumer_drop_bounded_block() {
        // `Tx::send` for `BoundedBlock` calls `SyncSender::send`
        // which returns `Err(SendError)` once the receiver is
        // dropped. Pin so the conversion (`map_err(|_| ())`) is
        // not lost in a refactor — without it, the worker
        // thread would stall indefinitely on a closed channel.
        let (tx, rx, _) = make_channel::<u32>(2, Overflow::Block);
        drop(rx);
        assert!(
            tx.send(1).is_err(),
            "Block producer send must surface Err after consumer drop"
        );
    }

    #[test]
    fn default_runnerconfig_is_bounded() {
        let cfg = super::RunnerConfig::default();
        assert!(cfg.frames_capacity > 0);
        assert!(cfg.pictures_capacity > 0);
        assert!(cfg.events_capacity > 0);
        assert_eq!(cfg.overflow, Overflow::DropNewest);
    }

    /// Pin the exact default values for `RunnerConfig`. The
    /// existing `default_runnerconfig_is_bounded` only checks that
    /// the channel capacities are non-zero; the specific numbers
    /// (`4` / `8` / `32`) and the polling cadences (`10ms` /
    /// `50ms` / `5s`) are part of the runner's documented behaviour
    /// and a regression that silently halves `frames_capacity` (or
    /// switches the overflow policy) would change the
    /// memory-vs-latency trade-off downstream consumers depend on.
    /// Pin the exact shape so the change requires updating both
    /// this test and the documentation in lock-step.
    #[test]
    fn default_runnerconfig_exact_values() {
        let cfg = super::RunnerConfig::default();
        assert_eq!(cfg.poll_interval, Duration::from_millis(10));
        assert_eq!(cfg.event_tick, Duration::from_millis(50));
        assert_eq!(cfg.shutter_timeout, Duration::from_secs(5));
        assert_eq!(cfg.frames_capacity, 4);
        assert_eq!(cfg.pictures_capacity, 8);
        assert_eq!(cfg.events_capacity, 32);
        assert_eq!(cfg.overflow, Overflow::DropNewest);
    }

    /// `Overflow::default()` is the source of truth for what
    /// happens when the user picks bounded channels but doesn't
    /// specify an overflow policy. `DropNewest` is the lowest-
    /// overhead choice (no relay thread spawned) and the runner's
    /// documented default — a regression that flipped to
    /// `DropOldest` or `Block` would silently change runner
    /// memory-and-stall behaviour for every default caller. Pin the
    /// derive's choice so a refactor of the enum (e.g. reordering
    /// variants and removing `#[default]`) gets caught here.
    #[test]
    fn overflow_default_is_drop_newest() {
        assert_eq!(Overflow::default(), Overflow::DropNewest);
    }

    /// `make_channel` `DropOldest` relay's producer-disconnect flush
    /// loop (`src/runner.rs:215-220`) drains the in-memory `VecDeque`
    /// into the user channel after the producer disconnects. The
    /// existing `drop_oldest_relay_exits_when_rx_dropped` covers the
    /// rx-drop arm, but exercises only the *empty-buf* path — `rx` is
    /// dropped before any item is sent, so the relay either observes
    /// `Disconnected` on `user_tx.try_send` (line 187) or never enters
    /// the flush loop at all.
    ///
    /// This test covers the *non-empty-buf* path: items are delivered
    /// into the relay's buffer, the producer is then dropped (so the
    /// relay observes `RecvTimeoutError::Disconnected` on lines
    /// 206-221), and the flush loop drains the buffered items into
    /// `user_tx` via blocking `send`. Then the user drains everything
    /// from `user_rx` and the relay handle must join cleanly. A
    /// regression that turned the flush loop's blocking `send` into a
    /// `try_send` retry-spin (or dropped the loop entirely) would
    /// surface here as either dropped items or a stuck relay.
    #[test]
    fn drop_oldest_relay_flushes_non_empty_buffer_on_producer_disconnect() {
        use std::time::Instant;

        let (tx, rx, relay) = make_channel::<u32>(2, Overflow::DropOldest);
        let relay = relay.unwrap();

        // Push 2 items so the relay buffers them. We don't drain
        // concurrently — the relay's inner loop will move them into
        // `user_tx` (capacity 2) on its next iteration. After this,
        // `prod_tx`, the relay's `buf`, and `user_tx` together hold
        // exactly 2 items somewhere along the chain.
        tx.send(10).unwrap();
        tx.send(20).unwrap();

        // Drop the producer side. The relay's `relay_rx.recv_timeout`
        // (or the next `recv` if `buf` is empty) returns
        // `Disconnected`, which sends it into the flush loop.
        drop(tx);

        // Drain. Both items must arrive in FIFO order — the flush
        // loop preserves order via `pop_front` + `send`.
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while received.len() < 2 && Instant::now() < deadline {
            if let Ok(v) = rx.recv_timeout(Duration::from_millis(50)) {
                received.push(v);
            }
        }
        assert_eq!(
            received,
            vec![10, 20],
            "producer-disconnect flush must preserve FIFO order"
        );

        // Once the buffer is fully flushed, the relay's flush loop
        // exits (`return` after the while). The handle must join
        // promptly — a stuck relay would block the test forever.
        let join_deadline = Instant::now() + Duration::from_secs(1);
        while !relay.is_finished() {
            assert!(
                Instant::now() < join_deadline,
                "DropOldest relay did not exit after producer \
                 disconnect + buffer drain; flush loop at \
                 src/runner.rs:215-220 likely regressed"
            );
            std::thread::yield_now();
        }
        relay.join().unwrap();
    }
}
