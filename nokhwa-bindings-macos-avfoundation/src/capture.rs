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
use crate::callback::{AVCaptureVideoCallback, FrameData};
use crate::device::AVCaptureDeviceWrapper;
use crate::session::{
    create_device_input, create_video_data_output, output_add_delegate, output_set_frame_format,
    session_add_input, session_add_output, session_begin_configuration,
    session_commit_configuration, session_is_interrupted, session_is_running, session_new,
    session_remove_input, session_remove_output, session_start, session_stop,
};
use nokhwa_core::{
    buffer::{Buffer, TimestampKind},
    error::NokhwaError,
    traits::{CameraDevice, FrameSource},
    types::{
        ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
        FrameFormat, KnownCameraControl, RequestedFormat,
    },
};
use objc2::rc::Retained;
use objc2_av_foundation::{AVCaptureDeviceInput, AVCaptureSession, AVCaptureVideoDataOutput};
use std::sync::mpsc::{Receiver, Sender};
use std::{borrow::Cow, ffi::CString, sync::Arc};

/// The backend struct that interfaces with `AVFoundation`.
/// Implements [`CameraDevice`] and [`FrameSource`].
/// # Quirks
/// - While working with `iOS` is allowed, it is not officially supported and may not work.
/// - You **must** call [`nokhwa_initialize`](crate::nokhwa_initialize) **before** doing anything with `AVFoundation`.
/// - This only works on 64 bit platforms.
/// - FPS adjustment does not work.
/// - If permission has not been granted and you call `init()` it will error.
pub struct AVFoundationCaptureDevice {
    device: AVCaptureDeviceWrapper,
    dev_input: Option<Retained<AVCaptureDeviceInput>>,
    session: Option<Retained<AVCaptureSession>>,
    data_out: Option<Retained<AVCaptureVideoDataOutput>>,
    data_collect: Option<AVCaptureVideoCallback>,
    info: CameraInfo,
    buffer_name: CString,
    format: CameraFormat,
    frame_buffer_receiver: Receiver<FrameData>,
    fbufsnd: Arc<Sender<FrameData>>,
}

impl AVFoundationCaptureDevice {
    /// Creates a new capture device using the `AVFoundation` backend. Indexes are gives to devices by the OS, and usually numbered by order of discovery.
    ///
    /// If `camera_format` is `None`, it will be spawned with with 640x480@15 FPS, MJPEG [`CameraFormat`] default.
    /// # Errors
    /// This function will error if the camera is currently busy or if `AVFoundation` can't read device information, or permission was not given by the user.
    pub fn new(index: &CameraIndex, req_fmt: RequestedFormat) -> Result<Self, NokhwaError> {
        let mut device = AVCaptureDeviceWrapper::new(index)?;

        let formats = device.supported_formats()?;
        let camera_fmt = req_fmt.fulfill(&formats).ok_or_else(|| {
            NokhwaError::open_device(
                index.to_string(),
                format!("Cannot fulfill request: {req_fmt}"),
            )
        })?;
        device.set_all(camera_fmt)?;

        let device_descriptor = device.info().clone();
        let buffername = CString::new(format!("{device_descriptor}_INDEX{index}_"))
            .map_err(|why| NokhwaError::structure("CString Buffername", why.to_string()))?;

        let (send, recv) = std::sync::mpsc::channel();
        Ok(AVFoundationCaptureDevice {
            device,
            dev_input: None,
            session: None,
            data_out: None,
            data_collect: None,
            info: device_descriptor,
            buffer_name: buffername,
            format: camera_fmt,
            frame_buffer_receiver: recv,
            fbufsnd: Arc::new(send),
        })
    }
}

impl AVFoundationCaptureDevice {
    /// Refreshes the cached camera format by querying the `AVFoundation` device.
    /// Kept as an inherent helper after the trait split; used internally by
    /// `open()` and `frame()`.
    fn refresh_camera_format(&mut self) -> Result<(), NokhwaError> {
        self.format = self.device.active_format()?;
        Ok(())
    }

    /// Look up a single control by its [`KnownCameraControl`] identifier.
    /// Kept as an inherent helper after the trait split.
    pub fn camera_control(
        &self,
        control: KnownCameraControl,
    ) -> Result<CameraControl, NokhwaError> {
        for ctrl in self.device.get_controls()? {
            if ctrl.control() == control {
                return Ok(ctrl);
            }
        }

        Err(NokhwaError::get_property(control.to_string(), "Not Found"))
    }
}

impl CameraDevice for AVFoundationCaptureDevice {
    fn backend(&self) -> ApiBackend {
        ApiBackend::AVFoundation
    }

    fn info(&self) -> &CameraInfo {
        &self.info
    }

    fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        self.device.get_controls()
    }

    fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        self.device.lock()?;
        let res = self.device.set_control(id, &value);
        self.device.unlock();
        res
    }
}

impl FrameSource for AVFoundationCaptureDevice {
    fn negotiated_format(&self) -> CameraFormat {
        self.format
    }

    fn set_format(&mut self, new_fmt: CameraFormat) -> Result<(), NokhwaError> {
        self.device.set_all(new_fmt)?;
        self.format = new_fmt;
        Ok(())
    }

    fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
        self.device.supported_formats()
    }

    fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
        let mut formats = self
            .device
            .supported_formats()?
            .into_iter()
            .map(|fmt: CameraFormat| fmt.format())
            .collect::<Vec<FrameFormat>>();
        formats.sort();
        formats.dedup();
        Ok(formats)
    }

    fn open(&mut self) -> Result<(), NokhwaError> {
        if self.is_open() {
            return Ok(());
        }
        self.refresh_camera_format()?;

        let input = create_device_input(self.device.inner())?;
        let session = session_new();
        session_begin_configuration(&session);
        session_add_input(&session, &input)?;

        self.device.set_all(self.format)?;

        let bufname = &self.buffer_name;
        let videocallback = AVCaptureVideoCallback::new(bufname, &self.fbufsnd)?;
        let output = create_video_data_output();
        output_add_delegate(&output, &videocallback)?;
        output_set_frame_format(&output, self.format.format())?;
        session_add_output(&session, &output)?;
        session_commit_configuration(&session);
        session_start(&session)?;

        self.dev_input = Some(input);
        self.session = Some(session);
        self.data_collect = Some(videocallback);
        self.data_out = Some(output);
        Ok(())
    }

    fn is_open(&self) -> bool {
        match (
            &self.session,
            &self.data_out,
            &self.data_collect,
            &self.dev_input,
        ) {
            (Some(session), Some(_), Some(_), Some(_)) => {
                !session_is_interrupted(session) && session_is_running(session)
            },
            _ => false,
        }
    }

    fn frame(&mut self) -> Result<Buffer, NokhwaError> {
        self.refresh_camera_format()?;
        let cfmt = self.format;
        let (bytes, _fmt, capture_ts) =
            self.frame_buffer_receiver
                .recv()
                .map_err(|why| NokhwaError::ReadFrameError {
                    message: why.to_string(),
                    format: Some(cfmt.format()),
                })?;
        let buffer = Buffer::from_vec_with_timestamp(
            cfmt.resolution(),
            bytes,
            cfmt.format(),
            capture_ts.map(|ts| (ts, TimestampKind::Presentation)),
        );
        self.frame_buffer_receiver.try_iter().for_each(drop);
        Ok(buffer)
    }

    fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
        match self.frame_buffer_receiver.recv() {
            Ok(recv) => {
                self.frame_buffer_receiver.try_iter().for_each(drop);
                Ok(Cow::from(recv.0))
            },
            Err(why) => Err(NokhwaError::ReadFrameError {
                message: why.to_string(),
                format: Some(self.format.format()),
            }),
        }
    }

    fn close(&mut self) -> Result<(), NokhwaError> {
        if !self.is_open() {
            return Ok(());
        }

        // `is_open()` above guarantees all three fields are `Some`; this
        // destructure folds the three separate guards into one fall-through
        // error path that should never fire in practice.
        let (Some(session), Some(output), Some(input)) =
            (&self.session, &self.data_out, &self.dev_input)
        else {
            return Err(NokhwaError::get_property(
                "AVCaptureSession/Output/Input",
                "Doesnt Exist",
            ));
        };

        // Stop the running session before mutating its topology, then bracket
        // the input/output removals in a begin/commit configuration block.
        // AVFoundation requires mutations to an `AVCaptureSession` to be wrapped
        // in `beginConfiguration`/`commitConfiguration`; removing inputs/outputs
        // from a still-running, unbracketed session is documented as producing
        // undefined results (the session can stall or log errors). `open()`
        // already adds them inside such a bracket — teardown must mirror it.
        session_stop(session);
        session_begin_configuration(session);
        session_remove_output(session, output);
        session_remove_input(session, input);
        session_commit_configuration(session);

        self.frame_buffer_receiver.try_iter().for_each(drop);
        self.dev_input = None;
        self.session = None;
        self.data_collect = None;
        self.data_out = None;

        Ok(())
    }
}

impl Drop for AVFoundationCaptureDevice {
    fn drop(&mut self) {
        let _ = self.close();
        self.device.unlock();
    }
}

// SAFETY: AVFoundationCaptureDevice is safe to Send (move) between threads because:
// - All access goes through &mut self, so after a move the new thread has exclusive
//   ownership — no aliasing across threads occurs. We do NOT implement Sync.
// - The Retained<AVCaptureDevice/Session/Input/Output> fields hold reference-counted
//   ObjC pointers. objc2 conservatively marks them !Send because Apple docs recommend
//   dispatching calls on a specific queue, but exclusive &mut self access satisfies
//   that constraint: only one thread touches them at a time.
// - The GCD DispatchQueue is designed for cross-thread use.
// - The *mut AnyObject delegate is only accessed during setup and by the GCD queue
//   callback; it is not touched directly after construction.
// - The mpsc Sender/Receiver are themselves Send.
unsafe impl Send for AVFoundationCaptureDevice {}
