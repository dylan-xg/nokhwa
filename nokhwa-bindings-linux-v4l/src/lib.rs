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
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

#[cfg(target_os = "linux")]
mod internal {
    use nokhwa_core::{
        buffer::{Buffer, TimestampKind},
        error::NokhwaError,
        traits::{CameraDevice, FrameSource},
        types::{
            ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo,
            ControlValueDescription, ControlValueSetter, FrameFormat, KnownCameraControl,
            KnownCameraControlFlag, RequestedFormat, Resolution,
        },
    };
    use std::{
        borrow::Cow,
        io::{self, ErrorKind},
    };
    use v4l::v4l_sys::{
        V4L2_CID_BACKLIGHT_COMPENSATION, V4L2_CID_BRIGHTNESS, V4L2_CID_CONTRAST, V4L2_CID_EXPOSURE,
        V4L2_CID_FOCUS_RELATIVE, V4L2_CID_GAIN, V4L2_CID_GAMMA, V4L2_CID_HUE,
        V4L2_CID_IRIS_RELATIVE, V4L2_CID_PAN_RELATIVE, V4L2_CID_SATURATION, V4L2_CID_SHARPNESS,
        V4L2_CID_TILT_RELATIVE, V4L2_CID_WHITE_BALANCE_TEMPERATURE, V4L2_CID_ZOOM_RELATIVE,
    };
    use v4l::{
        control::{Control, Flags, Type, Value},
        frameinterval::FrameIntervalEnum,
        framesize::FrameSizeEnum,
        io::traits::CaptureStream,
        prelude::MmapStream,
        video::{capture::Parameters, Capture},
        Device, Format, FourCC,
    };

    /// V4L2 control IDs in canonical [`KnownCameraControl`] index order.
    const V4L2_CONTROL_IDS: [u32; KnownCameraControl::STANDARD_COUNT] = [
        V4L2_CID_BRIGHTNESS,
        V4L2_CID_CONTRAST,
        V4L2_CID_HUE,
        V4L2_CID_SATURATION,
        V4L2_CID_SHARPNESS,
        V4L2_CID_GAMMA,
        V4L2_CID_WHITE_BALANCE_TEMPERATURE,
        V4L2_CID_BACKLIGHT_COMPENSATION,
        V4L2_CID_GAIN,
        V4L2_CID_PAN_RELATIVE,
        V4L2_CID_TILT_RELATIVE,
        V4L2_CID_ZOOM_RELATIVE,
        V4L2_CID_EXPOSURE,
        V4L2_CID_IRIS_RELATIVE,
        V4L2_CID_FOCUS_RELATIVE,
    ];

    /// Converts a [`KnownCameraControl`] into a V4L2 Control ID.
    #[must_use]
    pub fn known_camera_control_to_id(ctrl: KnownCameraControl) -> u32 {
        ctrl.to_platform_id(&V4L2_CONTROL_IDS)
    }

    /// Converts a V4L2 Control ID into a [`KnownCameraControl`].
    /// Unrecognised IDs are returned as `Other(id)`.
    #[must_use]
    pub fn id_to_known_camera_control(id: u32) -> KnownCameraControl {
        KnownCameraControl::from_platform_id(id, &V4L2_CONTROL_IDS)
    }

    /// query v4l2 cameras
    #[allow(clippy::unnecessary_wraps)]
    pub fn query() -> Result<Vec<CameraInfo>, NokhwaError> {
        Ok(v4l::context::enum_devices()
            .iter()
            .map(|node| {
                CameraInfo::new(
                    &node
                        .name()
                        .unwrap_or(node.path().to_string_lossy().into_owned()),
                    &format!("Video4Linux Device @ {}", node.path().to_string_lossy()),
                    "",
                    CameraIndex::Index(node.index() as u32),
                )
            })
            .collect())
    }

    type SharedDevice = std::sync::Arc<std::sync::Mutex<Device>>;
    type WeakSharedDevice = std::sync::Weak<std::sync::Mutex<Device>>;

    struct WeakSharedDeviceEntry {
        device: WeakSharedDevice,
        index: usize,
    }

    type SharedDeviceList = std::sync::OnceLock<std::sync::Mutex<Vec<WeakSharedDeviceEntry>>>;

    /// Global array that keep track of every Device that are currently open.
    /// This is used to open multiple handle to the same device.
    /// This is a workaround for the fact that the v4l2 backend does not support multiple handles to the same device.
    /// This replicate behavior of MF backend.
    /// The reference is a reference of Weak<Mutex<Device>>, so that the Device can be dropped when the last handle is closed.
    /// This list might need some cleanup because it will accumulate every device that is opened, this should not be a problem because the list should not grow too much.
    /// The list is also protected by a mutex, so it should be thread safe.
    static DEVICES: SharedDeviceList = std::sync::OnceLock::new();

    fn cleanup_dropped_devices(devices: &mut Vec<WeakSharedDeviceEntry>) {
        devices.retain(|entry| entry.device.strong_count() > 0);
    }

    fn new_shared_device(index: usize) -> Result<SharedDevice, NokhwaError> {
        let mut devices = DEVICES
            .get_or_init(|| std::sync::Mutex::new(Vec::new()))
            .lock()
            .map_err(|e| NokhwaError::InitializeError {
                backend: ApiBackend::Video4Linux,
                error: format!("Fail to lock global device list mutex: {e}"),
            })?;

        // do some cleanup, this will avoid here memory to grow forever
        // if for some reason someone has tons of camera plugged in
        cleanup_dropped_devices(&mut devices);

        if let Some(entry) = devices.iter().find(|entry| entry.index == index) {
            if let Some(device) = entry.device.upgrade() {
                return Ok(device);
            }
        }

        // Cleanup a second, the device we are interested might have been dropped during before upgrade call
        // For this point on we are assured that the device is not in the list
        cleanup_dropped_devices(&mut devices);

        // Let's be extra sure, this code should never panic, but maybe will help catch some race condition
        assert!(
            !devices.iter().any(|entry| entry.index == index),
            "Device {index} should not be in the list"
        );

        // Now we can open the device, and never run into a busy io error,
        // as long as the device isn't opened by other programs.
        let device = match Device::new(index) {
            Ok(dev) => dev,
            Err(why) => {
                return Err(NokhwaError::open_device(
                    index.to_string(),
                    format!("V4L2 Error: {why}"),
                ))
            },
        };

        let device = std::sync::Arc::new(std::sync::Mutex::new(device));
        devices.push(WeakSharedDeviceEntry {
            device: std::sync::Arc::downgrade(&device),
            index,
        });

        // Last check to be sure that every devices have a unique index
        // and that the data isn't corrupted
        if devices.len() > 1 {
            let indices: std::collections::HashSet<_> = devices.iter().map(|d| d.index).collect();
            assert_eq!(
                indices.len(),
                devices.len(),
                "Device list should not contain duplicate indexes"
            );
        }

        Ok(device)
    }

    /// Expand a V4L2 `FrameInterval` into the list of `CameraFormat`s it
    /// represents at the given resolution + frame format.
    ///
    /// V4L2 reports frame intervals as either:
    /// - `Discrete(n/d)` — accept only `n == 1` and yield `denominator` as FPS
    /// - `Stepwise { min, max, step }` — yield `min.numerator..=max.numerator`
    ///   step-by-`step.numerator` as FPS values, but only when min/max have
    ///   non-unit denominators (mirrors the historical guard from
    ///   `compatible_formats()` / `new()`).
    fn expand_frame_interval(
        interval: FrameIntervalEnum,
        resolution: Resolution,
        fmt: FrameFormat,
    ) -> Vec<CameraFormat> {
        match interval {
            FrameIntervalEnum::Discrete(dis) => {
                if dis.numerator == 1 {
                    vec![CameraFormat::new(resolution, fmt, dis.denominator)]
                } else {
                    vec![]
                }
            },
            FrameIntervalEnum::Stepwise(step) => {
                if step.max.denominator == 1 && step.min.denominator == 1 {
                    return vec![];
                }
                if step.step.numerator == 0 {
                    return vec![];
                }
                (step.min.numerator..=step.max.numerator)
                    .step_by(step.step.numerator as usize)
                    .map(|fps| CameraFormat::new(resolution, fmt, fps))
                    .collect()
            },
        }
    }

    fn get_device_format(device: &Device) -> Result<CameraFormat, NokhwaError> {
        match device.format() {
            Ok(format) => {
                let frame_format = fourcc_to_frameformat(format.fourcc)
                    .ok_or(NokhwaError::get_property("FrameFormat", "unsupported"))?;

                let fps = match device.params() {
                    Ok(params) => interval_to_fps(params.interval)?,
                    Err(why) => {
                        return Err(NokhwaError::get_property("V4L2 FrameRate", why.to_string()))
                    },
                };

                Ok(CameraFormat::new(
                    Resolution::new(format.width, format.height),
                    frame_format,
                    fps,
                ))
            },
            Err(why) => Err(NokhwaError::get_property("parameters", why.to_string())),
        }
    }

    /// The backend struct that interfaces with V4L2.
    /// Implements [`CameraDevice`] and [`FrameSource`].
    ///
    /// # Static-lifetime invariant
    ///
    /// `stream_handle` stores `MmapStream<'static>`, but `v4l::io::mmap::
    /// Stream<'a>`'s `'a` parameter really marks the lifetime of the kernel-
    /// mapped buffer slices (`Arena<'a>.bufs: Vec<&'a mut [u8]>`).
    ///
    /// Soundness: both `Stream` and its internal `Arena` each carry their own
    /// `Arc<Handle>` clone of the V4L2 file descriptor. The mmap'd buffers
    /// remain valid for as long as any `Arc<Handle>` clone is alive, so the
    /// slices in `Arena.bufs` cannot outlive the fd no matter what happens to
    /// this struct's own `device: SharedDevice` field. Extending `'a` to
    /// `'static` is therefore sound.
    ///
    /// The field order below (`stream_handle` before `device`) is cosmetic.
    /// Both orderings are sound: the `Arc<Handle>` clones inside the stream
    /// keep the fd alive, and `VIDIOC_STREAMOFF` is issued against that same
    /// `Arc<Handle>` on drop, so it runs correctly no matter when `device`
    /// is dropped.
    pub struct V4LCaptureDevice {
        stream_handle: Option<MmapStream<'static>>,
        device: SharedDevice,
        camera_format: CameraFormat,
        camera_info: CameraInfo,
    }

    // Compile-time assertion: `V4LCaptureDevice: 'static`. The `'static` bound
    // is required by `Box<dyn AnyDevice>` in the Layer 2 session machinery
    // (the `nokhwa_backend!` macro expansion plugs this type in there). This
    // guard catches regressions that would re-introduce a non-`'static` field
    // on our own struct — e.g. someone reverting to `MmapStream<'a>` with a
    // borrowed `'a` and dropping the transmute. Better a crisp build-time
    // error than a confusing macro-expansion error downstream.
    const _: () = {
        fn assert_static<T: 'static>() {}
        let _ = assert_static::<V4LCaptureDevice>;
    };

    impl V4LCaptureDevice {
        /// Creates a new capture device using the `V4L2` backend. Indexes are gives to devices by the OS, and usually numbered by order of discovery.
        /// # Errors
        /// This function will error if the camera is currently busy or if `V4L2` can't read device information.
        #[allow(clippy::too_many_lines)]
        pub fn new(index: &CameraIndex, cam_fmt: RequestedFormat) -> Result<Self, NokhwaError> {
            let index = index.clone();

            let shared_device = new_shared_device(index.as_index()? as usize)?;
            let device = shared_device
                .lock()
                .map_err(|e| NokhwaError::InitializeError {
                    backend: ApiBackend::Video4Linux,
                    error: format!("Fail to lock device mutex: {e}"),
                })?;

            // get all formats
            // get all fcc
            let mut camera_formats = vec![];

            let frame_formats = match device.enum_formats() {
                Ok(formats) => {
                    let mut frame_format_vec = vec![];
                    for fmt in &formats {
                        frame_format_vec.push(fmt.fourcc);
                    }
                    frame_format_vec.dedup();
                    Ok(frame_format_vec)
                },
                Err(why) => Err(NokhwaError::get_property("FrameFormat", why.to_string())),
            }?;

            for ff in frame_formats {
                let Some(framefmt) = fourcc_to_frameformat(ff) else {
                    continue;
                };
                let mut formats = device
                    .enum_framesizes(ff)
                    .map_err(|why| NokhwaError::get_property("ResolutionList", why.to_string()))?
                    .into_iter()
                    .flat_map(|x| match x.size {
                        FrameSizeEnum::Discrete(d) => [Resolution::new(d.width, d.height)].to_vec(),
                        FrameSizeEnum::Stepwise(s) => {
                            let mut v = Vec::new();
                            expand_stepwise_resolutions(
                                s.min_width,
                                s.max_width,
                                s.step_width,
                                s.min_height,
                                s.max_height,
                                s.step_height,
                                &mut v,
                            );
                            v
                        },
                    })
                    .flat_map(|res| {
                        device
                            .enum_frameintervals(ff, res.x(), res.y())
                            .unwrap_or_default()
                            .into_iter()
                            .flat_map(move |x| {
                                let res = Resolution::new(x.width, x.height);
                                expand_frame_interval(x.interval, res, framefmt)
                            })
                    })
                    .collect::<Vec<CameraFormat>>();
                camera_formats.append(&mut formats);
            }

            let format = cam_fmt
                .fulfill(&camera_formats)
                .ok_or(NokhwaError::get_property(
                    "CameraFormat",
                    "Failed to fulfill requested CameraFormat",
                ))?;

            let current_format = get_device_format(&device)?;

            if current_format.width() != format.width()
                || current_format.height() != format.height()
                || current_format.format() != format.format()
            {
                if let Err(why) = device.set_format(&Format::new(
                    format.width(),
                    format.height(),
                    frameformat_to_fourcc(format.format()),
                )) {
                    return Err(NokhwaError::set_property(
                        "Resolution, FrameFormat",
                        format.to_string(),
                        why.to_string(),
                    ));
                }
            }

            if current_format.frame_rate() != format.frame_rate() {
                if let Err(why) = device.set_params(&Parameters::with_fps(format.frame_rate())) {
                    return Err(NokhwaError::set_property(
                        "Frame rate",
                        format.frame_rate().to_string(),
                        why.to_string(),
                    ));
                }
            }

            let device_caps = device
                .query_caps()
                .map_err(|why| NokhwaError::get_property("Device Capabilities", why.to_string()))?;

            drop(device);

            let mut v4l2 = V4LCaptureDevice {
                camera_format: format,
                camera_info: CameraInfo::new(
                    &device_caps.card,
                    &device_caps.driver,
                    &format!("{} {:?}", device_caps.bus, device_caps.version),
                    index,
                ),
                device: shared_device,
                stream_handle: None,
            };

            v4l2.force_refresh_camera_format()?;
            if v4l2.negotiated_format() != format {
                return Err(NokhwaError::set_property(
                    "CameraFormat",
                    String::new(),
                    "Not same/Rejected",
                ));
            }

            Ok(v4l2)
        }

        fn lock_device(&self) -> Result<std::sync::MutexGuard<'_, Device>, NokhwaError> {
            self.device.lock().map_err(|e| NokhwaError::GeneralError {
                message: format!("Failed to lock device: {e}"),
                backend: Some(ApiBackend::Video4Linux),
            })
        }

        fn get_resolution_list(&self, fourcc: FrameFormat) -> Result<Vec<Resolution>, NokhwaError> {
            let format = frameformat_to_fourcc(fourcc);

            match self.lock_device()?.enum_framesizes(format) {
                Ok(frame_sizes) => {
                    let mut resolutions = vec![];
                    for frame_size in frame_sizes {
                        match frame_size.size {
                            FrameSizeEnum::Discrete(dis) => {
                                resolutions.push(Resolution::new(dis.width, dis.height));
                            },
                            FrameSizeEnum::Stepwise(step) => {
                                // V4L Stepwise advertises a (min, max,
                                // step) triple — every (min + k*step,
                                // min + k*step) pair up to max is
                                // legal. Naive full enumeration on a
                                // 1×1-step / 4096×4096-max driver
                                // produces millions of synthetic
                                // resolutions, so we expose:
                                //
                                // 1. the min and max endpoints
                                //    (always legal),
                                // 2. each `COMMON_RESOLUTIONS` preset
                                //    that fits inside the (min..=max)
                                //    box AND aligns to the advertised
                                //    width/height step.
                                //
                                // Drivers still accept arbitrary
                                // intermediate resolutions via
                                // `set_format`; this list is for UI
                                // surfaces that need a sane shortlist.
                                expand_stepwise_resolutions(
                                    step.min_width,
                                    step.max_width,
                                    step.step_width,
                                    step.min_height,
                                    step.max_height,
                                    step.step_height,
                                    &mut resolutions,
                                );
                            },
                        }
                    }
                    Ok(resolutions)
                },
                Err(why) => Err(NokhwaError::get_property("Resolutions", why.to_string())),
            }
        }

        /// Force refreshes the inner [`CameraFormat`] state.
        /// # Errors
        /// If the internal representation in the driver is invalid, this will error.
        pub fn force_refresh_camera_format(&mut self) -> Result<(), NokhwaError> {
            let camera_format = get_device_format(&*self.lock_device()?)?;
            self.camera_format = camera_format;
            Ok(())
        }
    }

    impl CameraDevice for V4LCaptureDevice {
        fn backend(&self) -> ApiBackend {
            ApiBackend::Video4Linux
        }

        fn info(&self) -> &CameraInfo {
            &self.camera_info
        }

        #[allow(clippy::cast_possible_wrap)]
        fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
            let device = self.lock_device()?;
            let camera_ctrls = device
                .query_controls()
                .map_err(|why| NokhwaError::get_property("V4L2 Controls", why.to_string()))?
                .into_iter()
                .map(|desc| {
                    let id_as_kcc = id_to_known_camera_control(desc.id);
                    let ctrl_current = device.control(desc.id)?.value;

                    let ctrl_value_desc = match (desc.typ, ctrl_current) {
                        (
                            Type::Integer
                            | Type::Integer64
                            | Type::Menu
                            | Type::U8
                            | Type::U16
                            | Type::U32
                            | Type::IntegerMenu,
                            Value::Integer(current),
                        ) => ControlValueDescription::IntegerRange {
                            min: desc.minimum,
                            max: desc.maximum,
                            value: current,
                            // desc.step is u64; widen into i64 which is
                            // what `ControlValueDescription` expects.
                            step: i64::try_from(desc.step).unwrap_or(i64::MAX),
                            default: desc.default,
                        },
                        (Type::Boolean, Value::Boolean(current)) => {
                            ControlValueDescription::Boolean {
                                value: current,
                                default: desc.default != 0,
                            }
                        },

                        (Type::String, Value::String(current)) => ControlValueDescription::String {
                            value: current,
                            default: None,
                        },
                        (ty, val) => {
                            return Err(io::Error::new(
                                ErrorKind::Unsupported,
                                format!(
                                "v4l control descriptor type {ty:?} does not match value {val:?}; \
                                     unsupported variant"
                            ),
                            ))
                        },
                    };

                    // V4L2 distinguishes DISABLED (permanently unusable) from
                    // INACTIVE (temporarily gated by another control, e.g.
                    // AUTO_GAIN=on hides GAIN). `KnownCameraControlFlag` has no
                    // `Inactive` variant, so we map both to `Disabled` and rely
                    // on the dedup below to avoid emitting it twice when a
                    // descriptor happens to carry both bits.
                    let is_readonly = desc
                        .flags
                        .intersects(Flags::READ_ONLY)
                        .then_some(KnownCameraControlFlag::ReadOnly);
                    let is_writeonly = desc
                        .flags
                        .intersects(Flags::WRITE_ONLY)
                        .then_some(KnownCameraControlFlag::WriteOnly);
                    let is_volatile = desc
                        .flags
                        .intersects(Flags::VOLATILE)
                        .then_some(KnownCameraControlFlag::Volatile);
                    let is_disabled = desc
                        .flags
                        .intersects(Flags::DISABLED | Flags::INACTIVE)
                        .then_some(KnownCameraControlFlag::Disabled);
                    let flags_vec = [is_readonly, is_writeonly, is_volatile, is_disabled]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<KnownCameraControlFlag>>();

                    Ok(CameraControl::new(
                        id_as_kcc,
                        desc.name,
                        ctrl_value_desc,
                        flags_vec,
                        !desc.flags.intersects(Flags::INACTIVE),
                    ))
                })
                .filter_map(Result::ok)
                .collect::<Vec<CameraControl>>();
            Ok(camera_ctrls)
        }

        fn set_control(
            &mut self,
            id: KnownCameraControl,
            value: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            let conv_value = match value.clone() {
                ControlValueSetter::None => Value::None,
                ControlValueSetter::Integer(i) => Value::Integer(i),
                ControlValueSetter::Boolean(b) => Value::Boolean(b),
                ControlValueSetter::String(s) => Value::String(s),
                ControlValueSetter::Bytes(b) => Value::CompoundU8(b),
                v => {
                    return Err(NokhwaError::set_property(
                        id.to_string(),
                        v.to_string(),
                        "not supported",
                    ))
                },
            };
            let v4l_id = known_camera_control_to_id(id);
            let device = self.lock_device()?;
            device
                .set_control(Control {
                    id: v4l_id,
                    value: conv_value,
                })
                .map_err(|why| {
                    NokhwaError::set_property(id.to_string(), format!("{value:?}"), why.to_string())
                })?;

            // Read-back verification, but only when the control supports a
            // meaningful read-back. WRITE_ONLY controls reject
            // `VIDIOC_G_EXT_CTRLS` with EACCES (so the read would always
            // fail), and VOLATILE controls report a hardware-updated value
            // that legitimately differs from what we just wrote — for both,
            // a read-back mismatch is expected and must NOT be treated as a
            // rejected write. The kernel already errored above if the write
            // itself was rejected.
            let skip_verify = device
                .query_controls()
                .ok()
                .and_then(|descs| descs.into_iter().find(|d| d.id == v4l_id))
                .is_some_and(|d| d.flags.intersects(Flags::WRITE_ONLY | Flags::VOLATILE));
            drop(device);
            if skip_verify {
                return Ok(());
            }

            let control = self.camera_control(id)?;
            if control.value() != value {
                return Err(NokhwaError::set_property(
                    id.to_string(),
                    format!("{value:?}"),
                    "Rejected",
                ));
            }
            Ok(())
        }
    }

    impl FrameSource for V4LCaptureDevice {
        fn negotiated_format(&self) -> CameraFormat {
            self.camera_format
        }

        fn set_format(&mut self, new_fmt: CameraFormat) -> Result<(), NokhwaError> {
            let (prev_format, prev_fps) = {
                let device = self.lock_device()?;
                let prev_format = match Capture::format(&*device) {
                    Ok(fmt) => fmt,
                    Err(why) => {
                        return Err(NokhwaError::get_property(
                            "Resolution, FrameFormat",
                            why.to_string(),
                        ))
                    },
                };
                let prev_fps = match Capture::params(&*device) {
                    Ok(fps) => fps,
                    Err(why) => {
                        return Err(NokhwaError::get_property("Frame rate", why.to_string()))
                    },
                };
                (prev_format, prev_fps)
            };

            // Tear down any live stream *before* changing the format. V4L2
            // rejects `VIDIOC_S_FMT` / `VIDIOC_REQBUFS` while a stream is
            // active (EBUSY), and `open()` allocates the new `MmapStream`
            // before dropping the old one — so without this the new buffers
            // would be requested while the old arena is still mapped and
            // streaming. `close()` drops the handle, whose `Drop` issues
            // `VIDIOC_STREAMOFF` and munmaps the arena.
            let was_streaming = self.stream_handle.is_some();
            if was_streaming {
                self.close()?;
            }

            let v4l_fcc = frameformat_to_fourcc(new_fmt.format());

            let format = Format::new(new_fmt.width(), new_fmt.height(), v4l_fcc);
            let frame_rate = Parameters::with_fps(new_fmt.frame_rate());

            {
                let device = self.lock_device()?;
                if let Err(why) = Capture::set_format(&*device, &format) {
                    return Err(NokhwaError::set_property(
                        "Resolution, FrameFormat",
                        format.to_string(),
                        why.to_string(),
                    ));
                }
                if let Err(why) = Capture::set_params(&*device, &frame_rate) {
                    return Err(NokhwaError::set_property(
                        "Frame rate",
                        frame_rate.to_string(),
                        why.to_string(),
                    ));
                }
            }

            if was_streaming {
                if let Err(why) = self.open() {
                    // Undo: restore the previous format/params, then try to
                    // re-acquire the stream the caller had before the failed
                    // re-negotiation so the device isn't left silently closed.
                    {
                        let device = self.lock_device()?;
                        if let Err(why) = Capture::set_format(&*device, &prev_format) {
                            return Err(NokhwaError::set_property(
                                format!("Attempt undo due to stream acquisition failure with error {why}. Resolution, FrameFormat"),
                                prev_format.to_string(),
                                why.to_string(),
                            ));
                        }
                        if let Err(why) = Capture::set_params(&*device, &prev_fps) {
                            return Err(NokhwaError::set_property(
                                format!("Attempt undo due to stream acquisition failure with error {why}. Frame rate"),
                                prev_fps.to_string(),
                                why.to_string(),
                            ));
                        }
                    }
                    let _ = self.open();
                    return Err(why);
                }
            }
            self.camera_format = new_fmt;

            self.force_refresh_camera_format()?;
            if self.camera_format != new_fmt {
                return Err(NokhwaError::set_property(
                    "CameraFormat",
                    new_fmt.to_string(),
                    "Rejected",
                ));
            }

            Ok(())
        }

        fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
            let fourccs = self.compatible_fourcc()?;
            let mut out: Vec<CameraFormat> = Vec::new();
            for fourcc in fourccs {
                let format = frameformat_to_fourcc(fourcc);
                let resolutions = self.get_resolution_list(fourcc)?;
                for res in resolutions {
                    match self
                        .lock_device()?
                        .enum_frameintervals(format, res.width(), res.height())
                    {
                        Ok(intervals) => {
                            for interval in intervals {
                                out.extend(expand_frame_interval(interval.interval, res, fourcc));
                            }
                        },
                        Err(why) => {
                            return Err(NokhwaError::get_property("Frame rate", why.to_string()))
                        },
                    }
                }
            }
            Ok(out)
        }

        fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
            match self.lock_device()?.enum_formats() {
                Ok(formats) => {
                    let mut frame_format_vec = vec![];
                    for format in formats {
                        if let Some(ff) = fourcc_to_frameformat(format.fourcc) {
                            frame_format_vec.push(ff);
                        }
                    }
                    frame_format_vec.sort();
                    frame_format_vec.dedup();
                    Ok(frame_format_vec)
                },
                Err(why) => Err(NokhwaError::get_property("FrameFormat", why.to_string())),
            }
        }

        fn open(&mut self) -> Result<(), NokhwaError> {
            // Calling `open()` when a stream already exists tears the old
            // stream down (its `Drop` issues `VIDIOC_STREAMOFF` and munmaps
            // the arena) and replaces it with a fresh one. This reset is
            // intentional — the `FrameSource` contract permits it.
            // Disable mut warning, since mut is only required when not using arena buffers
            #[allow(unused_mut)]
            let mut stream =
                match MmapStream::new(&*self.lock_device()?, v4l::buffer::Type::VideoCapture) {
                    Ok(s) => s,
                    Err(why) => {
                        return Err(NokhwaError::OpenStreamError {
                            message: why.to_string(),
                            backend: Some(ApiBackend::Video4Linux),
                        })
                    },
                };

            // Explicitly start now, or won't work with the RPi. As a consequence, buffers will only be used as required.
            // WARNING: This will cause drop of half of the frames
            #[cfg(feature = "no-arena-buffer")]
            match stream.start() {
                Ok(s) => s,
                Err(why) => {
                    return Err(NokhwaError::OpenStreamError {
                        message: why.to_string(),
                        backend: Some(ApiBackend::Video4Linux),
                    })
                },
            }

            // SAFETY: See the `'static` invariant doc on `V4LCaptureDevice`.
            // Briefly: `MmapStream` and its `Arena` each hold an `Arc<Handle>`
            // clone of the V4L2 fd, so the mmap'd slices in `Arena<'a>.bufs`
            // live as long as the stream itself. No `&'static` slice ever
            // leaks out: `MmapStream::next` reborrows through `&mut self`.
            let stream =
                unsafe { std::mem::transmute::<MmapStream<'_>, MmapStream<'static>>(stream) };
            self.stream_handle = Some(stream);
            Ok(())
        }

        fn is_open(&self) -> bool {
            self.stream_handle.is_some()
        }

        fn frame(&mut self) -> Result<Buffer, NokhwaError> {
            let cam_fmt = self.camera_format;
            match &mut self.stream_handle {
                Some(sh) => match sh.next() {
                    Ok((data, meta)) => {
                        let wall_ts = monotonic_to_wallclock(meta.timestamp);
                        // The mmap buffer is sized to the driver's maximum
                        // image size; only the first `bytesused` bytes are
                        // the current frame. Compressed formats (MJPEG) fill
                        // far less than the allocation, so handing the full
                        // slice downstream appends stale padding that corrupts
                        // decoding. Clamp in case a driver over-reports.
                        let used = (meta.bytesused as usize).min(data.len());
                        Ok(Buffer::with_timestamp(
                            cam_fmt.resolution(),
                            &data[..used],
                            cam_fmt.format(),
                            wall_ts.map(|ts| (ts, TimestampKind::WallClock)),
                        ))
                    },
                    Err(why) => Err(NokhwaError::ReadFrameError {
                        message: why.to_string(),
                        format: Some(cam_fmt.format()),
                    }),
                },
                None => Err(NokhwaError::read_frame("Stream Not Started")),
            }
        }

        fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
            let cam_fmt_format = self.camera_format.format();
            match &mut self.stream_handle {
                Some(sh) => match sh.next() {
                    Ok((data, meta)) => {
                        // See `frame()`: clamp to `bytesused` so compressed
                        // frames don't carry the mmap buffer's stale padding.
                        let used = (meta.bytesused as usize).min(data.len());
                        Ok(Cow::Borrowed(&data[..used]))
                    },
                    Err(why) => Err(NokhwaError::ReadFrameError {
                        message: why.to_string(),
                        format: Some(cam_fmt_format),
                    }),
                },
                None => Err(NokhwaError::read_frame("Stream Not Started")),
            }
        }

        fn close(&mut self) -> Result<(), NokhwaError> {
            if self.stream_handle.is_some() {
                self.stream_handle = None;
            }
            Ok(())
        }
    }

    impl V4LCaptureDevice {
        /// Look up a single control by its [`KnownCameraControl`] identifier.
        /// Kept as an inherent helper after the trait split; used internally by
        /// `set_control` to verify writes.
        pub fn camera_control(
            &self,
            control: KnownCameraControl,
        ) -> Result<CameraControl, NokhwaError> {
            let controls = self.controls()?;
            for supported_control in controls {
                if supported_control.control() == control {
                    return Ok(supported_control);
                }
            }
            Err(NokhwaError::get_property(
                control.to_string(),
                "not found/not supported",
            ))
        }
    }

    fn fourcc_to_frameformat(fourcc: FourCC) -> Option<FrameFormat> {
        // The V4L2 kernel wire token for grayscale is `GREY`
        // (`V4L2_PIX_FMT_GREY`), but nokhwa-core's canonical token is `GRAY`.
        // Translate at this boundary so a grayscale camera's formats are not
        // silently dropped during enumeration.
        if &fourcc.repr == b"GREY" {
            return Some(FrameFormat::GRAY);
        }
        FrameFormat::from_fourcc(fourcc.str().ok()?)
    }

    fn frameformat_to_fourcc(format: FrameFormat) -> FourCC {
        // See `fourcc_to_frameformat`: emit the kernel's `GREY` token for
        // grayscale rather than nokhwa-core's canonical `GRAY`, otherwise the
        // ioctl rejects the unrecognised fourcc.
        if format == FrameFormat::GRAY {
            return FourCC::new(b"GREY");
        }
        FourCC::new(
            format
                .to_fourcc()
                .as_bytes()
                .try_into()
                .expect("fourcc is always 4 bytes"),
        )
    }

    /// Common (width, height) presets exposed inside a Stepwise advertisement
    /// when they (a) fit the (min..=max) box and (b) align to the advertised
    /// width/height step. Ordered ascending by area.
    const COMMON_RESOLUTIONS: &[(u32, u32)] = &[
        (320, 240),
        (640, 480),
        (800, 600),
        (1024, 768),
        (1280, 720),
        (1280, 960),
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
    ];

    /// Append every legal resolution from a V4L2 Stepwise advertisement
    /// that we want to surface. Always pushes the (min, min) and (max, max)
    /// endpoints; in between, pushes any [`COMMON_RESOLUTIONS`] preset that
    /// (a) fits inside the (min..=max) box on both axes and (b) aligns to
    /// the advertised step on both axes. Step values of 0 are treated as
    /// "any" (= alignment OK), matching what V4L2 drivers do in practice
    /// when they advertise a continuous range.
    ///
    /// Output is in ascending area order with duplicates suppressed (a
    /// preset that coincides with an endpoint is emitted once).
    fn expand_stepwise_resolutions(
        min_w: u32,
        max_w: u32,
        step_w: u32,
        min_h: u32,
        max_h: u32,
        step_h: u32,
        out: &mut Vec<Resolution>,
    ) {
        let push_unique = |v: &mut Vec<Resolution>, r: Resolution| {
            if !v.contains(&r) {
                v.push(r);
            }
        };
        push_unique(out, Resolution::new(min_w, min_h));
        let aligns = |v: u32, base: u32, step: u32| step == 0 || (v - base).is_multiple_of(step);
        for &(w, h) in COMMON_RESOLUTIONS {
            if w >= min_w
                && w <= max_w
                && h >= min_h
                && h <= max_h
                && aligns(w, min_w, step_w)
                && aligns(h, min_h, step_h)
            {
                push_unique(out, Resolution::new(w, h));
            }
        }
        push_unique(out, Resolution::new(max_w, max_h));
    }

    /// Decode a V4L2 `Fraction` representing *seconds per frame* into
    /// integer frames per second.
    ///
    /// V4L2's `v4l2_fract` (`StreamParams::interval`) carries the
    /// frame interval (period) as `numerator / denominator` seconds.
    /// 30 fps is therefore the standard form `{1, 30}`. We accept
    /// only `numerator == 1` and return the denominator as `u32`
    /// fps. Anything else — fractional rates the [`CameraFormat`]
    /// surface cannot represent (e.g. `{1001, 30000}` for NTSC's
    /// 29.97 fps), or unreduced forms like `{2, 60}` — surfaces as
    /// `Err(NokhwaError::GetPropertyError)`.
    ///
    /// Split out from [`get_device_format`] so the contract can be
    /// pinned without a real `v4l::Device`. The previous inline
    /// shape carried a dead `else` branch that the upstream
    /// `numerator != 1` guard made unreachable.
    fn interval_to_fps(interval: v4l::Fraction) -> Result<u32, NokhwaError> {
        if interval.numerator != 1 {
            return Err(NokhwaError::get_property(
                "V4L2 FrameRate",
                format!(
                    "Framerate not whole number: {} / {}",
                    interval.denominator, interval.numerator
                ),
            ));
        }
        Ok(interval.denominator)
    }

    /// Convert a V4L2 `CLOCK_MONOTONIC` timestamp to a wallclock Duration since `UNIX_EPOCH`.
    fn monotonic_to_wallclock(ts: v4l::Timestamp) -> Option<std::time::Duration> {
        let frame_mono = std::time::Duration::from(ts);
        if frame_mono.is_zero() {
            return None;
        }

        let mut mono_now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut wall_now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: passing valid pointers to kernel clock_gettime
        unsafe {
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut mono_now);
            libc::clock_gettime(libc::CLOCK_REALTIME, &raw mut wall_now);
        }
        let mono_now = std::time::Duration::new(mono_now.tv_sec as u64, mono_now.tv_nsec as u32);
        let wall_now = std::time::Duration::new(wall_now.tv_sec as u64, wall_now.tv_nsec as u32);

        // frame_age = how long ago the frame was captured (monotonic delta)
        let frame_age = mono_now.checked_sub(frame_mono)?;
        wall_now.checked_sub(frame_age)
    }

    #[cfg(test)]
    mod tests {
        use super::{
            expand_frame_interval, expand_stepwise_resolutions, fourcc_to_frameformat,
            frameformat_to_fourcc, id_to_known_camera_control, interval_to_fps,
            known_camera_control_to_id, monotonic_to_wallclock, FrameFormat, KnownCameraControl,
            Resolution, V4L2_CONTROL_IDS,
        };
        use v4l::fraction::Fraction;
        use v4l::frameinterval::{FrameIntervalEnum, Stepwise};
        use v4l::v4l_sys::{
            V4L2_CID_BACKLIGHT_COMPENSATION, V4L2_CID_BRIGHTNESS, V4L2_CID_CONTRAST,
            V4L2_CID_EXPOSURE, V4L2_CID_FOCUS_RELATIVE, V4L2_CID_GAIN, V4L2_CID_GAMMA,
            V4L2_CID_HUE, V4L2_CID_IRIS_RELATIVE, V4L2_CID_PAN_RELATIVE, V4L2_CID_SATURATION,
            V4L2_CID_SHARPNESS, V4L2_CID_TILT_RELATIVE, V4L2_CID_WHITE_BALANCE_TEMPERATURE,
            V4L2_CID_ZOOM_RELATIVE,
        };
        use v4l::FourCC;
        use v4l::Timestamp;

        fn run(
            min_w: u32,
            max_w: u32,
            step_w: u32,
            min_h: u32,
            max_h: u32,
            step_h: u32,
        ) -> Vec<Resolution> {
            let mut out = Vec::new();
            expand_stepwise_resolutions(min_w, max_w, step_w, min_h, max_h, step_h, &mut out);
            out
        }

        #[test]
        fn endpoints_only_when_no_presets_fit() {
            // (max=300x200) is below the smallest preset (320x240).
            let out = run(64, 300, 1, 64, 200, 1);
            assert_eq!(
                out,
                vec![Resolution::new(64, 64), Resolution::new(300, 200)]
            );
        }

        #[test]
        fn min_equals_max_emits_single_endpoint() {
            // Degenerate Stepwise: a Discrete in disguise. Avoid emitting
            // the same point twice.
            let out = run(640, 640, 0, 480, 480, 0);
            assert_eq!(out, vec![Resolution::new(640, 480)]);
        }

        #[test]
        fn presets_in_range_with_step_one_all_pass() {
            // 320x240..=4096x4096 step 1: every preset that fits passes.
            // min (320,240) coincides with the first preset, so dedup
            // gives 9 distinct presets + the (4096,4096) max = 10.
            let out = run(320, 4096, 1, 240, 4096, 1);
            assert_eq!(out.len(), 10);
            assert_eq!(out.first(), Some(&Resolution::new(320, 240)));
            assert_eq!(out.last(), Some(&Resolution::new(4096, 4096)));
            assert!(out.contains(&Resolution::new(1280, 720)));
            assert!(out.contains(&Resolution::new(1920, 1080)));
        }

        #[test]
        fn step_misalignment_drops_preset() {
            // step_w = 16, min_w = 320: 1280 (= 320 + 60*16) aligns,
            // 1920 (= 320 + 100*16) aligns, but 800 (= 320 + 30*16)
            // also aligns. Pick a step that filters: step_w = 100,
            // min_w = 320 → only 320, 420, 520, ... ; 1280 is 320 + 9.6*100
            // → does not align. 1920 is 320 + 16*100 → aligns. 2560 →
            // 320 + 22.4*100 → does not align.
            let out = run(320, 4096, 100, 240, 4096, 1);
            assert!(out.contains(&Resolution::new(1920, 1080)));
            assert!(!out.contains(&Resolution::new(1280, 720)));
            assert!(!out.contains(&Resolution::new(2560, 1440)));
        }

        #[test]
        fn out_of_range_presets_excluded() {
            // max 1280x720: presets above must not appear.
            let out = run(320, 1280, 1, 240, 720, 1);
            assert!(out.contains(&Resolution::new(1280, 720)));
            assert!(!out.contains(&Resolution::new(1920, 1080)));
            assert!(!out.contains(&Resolution::new(3840, 2160)));
        }

        // V4L2_CONTROL_IDS contract: each row maps a canonical
        // KnownCameraControl index (0..=14) to the matching V4L2_CID_*
        // constant. If the order ever drifts, every standard control
        // gets the wrong CID — set_control / camera_control would
        // silently issue VIDIOC_S_CTRL on an unrelated control. Pin
        // the table.
        #[test]
        fn v4l2_control_ids_table_order_matches_known_camera_control_index() {
            let expected: [(KnownCameraControl, u32); KnownCameraControl::STANDARD_COUNT] = [
                (KnownCameraControl::Brightness, V4L2_CID_BRIGHTNESS),
                (KnownCameraControl::Contrast, V4L2_CID_CONTRAST),
                (KnownCameraControl::Hue, V4L2_CID_HUE),
                (KnownCameraControl::Saturation, V4L2_CID_SATURATION),
                (KnownCameraControl::Sharpness, V4L2_CID_SHARPNESS),
                (KnownCameraControl::Gamma, V4L2_CID_GAMMA),
                (
                    KnownCameraControl::WhiteBalance,
                    V4L2_CID_WHITE_BALANCE_TEMPERATURE,
                ),
                (
                    KnownCameraControl::BacklightComp,
                    V4L2_CID_BACKLIGHT_COMPENSATION,
                ),
                (KnownCameraControl::Gain, V4L2_CID_GAIN),
                (KnownCameraControl::Pan, V4L2_CID_PAN_RELATIVE),
                (KnownCameraControl::Tilt, V4L2_CID_TILT_RELATIVE),
                (KnownCameraControl::Zoom, V4L2_CID_ZOOM_RELATIVE),
                (KnownCameraControl::Exposure, V4L2_CID_EXPOSURE),
                (KnownCameraControl::Iris, V4L2_CID_IRIS_RELATIVE),
                (KnownCameraControl::Focus, V4L2_CID_FOCUS_RELATIVE),
            ];
            for (idx, (ctrl, cid)) in expected.iter().enumerate() {
                assert_eq!(
                    ctrl.as_index(),
                    Some(idx as u8),
                    "expected canonical index {idx} for {ctrl:?}"
                );
                assert_eq!(
                    V4L2_CONTROL_IDS[idx], *cid,
                    "V4L2_CONTROL_IDS[{idx}] should be the CID for {ctrl:?}"
                );
            }
        }

        #[test]
        fn known_camera_control_to_id_round_trips_for_every_standard_control() {
            for idx in 0..KnownCameraControl::STANDARD_COUNT {
                let ctrl = KnownCameraControl::from_index(
                    u8::try_from(idx).expect("STANDARD_COUNT < 256"),
                )
                .expect("from_index in range");
                let cid = known_camera_control_to_id(ctrl);
                let back = id_to_known_camera_control(cid);
                assert_eq!(back, ctrl, "round-trip failed for {ctrl:?} (cid={cid})");
            }
        }

        #[test]
        fn id_to_known_camera_control_unknown_returns_other() {
            // 0xFFFF_FFFF is not a real V4L2 CID — the table must
            // fall through to Other(id).
            let unknown: u32 = 0xFFFF_FFFF;
            match id_to_known_camera_control(unknown) {
                KnownCameraControl::Other(v) => assert_eq!(v, u128::from(unknown)),
                other => panic!("expected Other({unknown}), got {other:?}"),
            }
        }

        #[test]
        fn known_camera_control_to_id_other_truncates_to_u32() {
            // Round-trip an Other(_) through to_platform_id: the stored
            // u128 is truncated to u32 (V4L2 CIDs are u32 by definition).
            let raw: u128 = 0xDEAD_BEEF;
            let ctrl = KnownCameraControl::Other(raw);
            let id = known_camera_control_to_id(ctrl);
            assert_eq!(id, raw as u32);
            // And the reverse path resurfaces an Other for an
            // unrecognised CID — i.e. callers see the same variant
            // discriminant either side of the FFI boundary.
            let back = id_to_known_camera_control(id);
            assert_eq!(back, KnownCameraControl::Other(u128::from(id)));
        }

        #[test]
        fn monotonic_to_wallclock_zero_timestamp_returns_none() {
            // Drivers that don't fill in V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC
            // (or buffers that haven't been timestamped at all) leave the
            // timestamp at all-zero. The conversion must reject these
            // up-front rather than treating "0s since boot" as a valid
            // capture moment — an honest "we don't know" beats a wallclock
            // pinned to the kernel's boot epoch.
            assert_eq!(monotonic_to_wallclock(Timestamp::new(0, 0)), None);
        }

        #[test]
        fn monotonic_to_wallclock_future_timestamp_returns_none() {
            // If the kernel ever hands us a frame timestamped *after*
            // CLOCK_MONOTONIC's current reading (clock skew across
            // subsystems, a reset clocksource, a buggy emulator), the
            // function must return None rather than panic on subtraction
            // overflow or yield a nonsensical "frame from the future"
            // wallclock. Use sec=i64::MAX to guarantee the timestamp is
            // far ahead of any real CLOCK_MONOTONIC value at runtime.
            let far_future = Timestamp::new(i64::MAX, 0);
            assert_eq!(
                monotonic_to_wallclock(far_future),
                None,
                "future-monotonic timestamp must yield None, not panic or a future wallclock"
            );
        }

        #[test]
        fn monotonic_to_wallclock_recent_timestamp_close_to_current_realtime() {
            // Happy path: a timestamp captured "just now" must produce a
            // wallclock duration very close to the current realtime
            // reading. The function reads CLOCK_MONOTONIC and CLOCK_REALTIME
            // independently, so there's a small race window between the
            // read inside the function and our snapshot here — but it's
            // tightly bounded. We pin a tolerance of 1 second to give
            // headroom for a slow CI host while still catching regressions
            // that swap the clock IDs or invert the subtraction direction.
            let mut mono_now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let mut wall_now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: passing valid pointers to kernel clock_gettime
            unsafe {
                libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut mono_now);
                libc::clock_gettime(libc::CLOCK_REALTIME, &raw mut wall_now);
            }
            let mono_now_ts = Timestamp::new(mono_now.tv_sec, mono_now.tv_nsec / 1000);
            let wall_now_dur =
                std::time::Duration::new(wall_now.tv_sec as u64, wall_now.tv_nsec as u32);

            let observed = monotonic_to_wallclock(mono_now_ts)
                .expect("recent monotonic timestamp must produce a wallclock");

            let diff = observed.abs_diff(wall_now_dur);
            assert!(
                diff < std::time::Duration::from_secs(1),
                "wallclock conversion drifted >1s from realtime: observed={observed:?}, wall_now={wall_now_dur:?}, diff={diff:?}"
            );
        }

        /// `frameformat_to_fourcc` and `fourcc_to_frameformat` are the
        /// thin shims that bridge `v4l::FourCC` ↔ `FrameFormat` at every
        /// V4L2 ioctl boundary (`set_format`, enumerate, frame metadata).
        /// Pin the round-trip so a future tweak to `FrameFormat::to_fourcc`
        /// or `from_fourcc` cannot silently break v4l's wire-level
        /// translation.
        #[test]
        fn frameformat_fourcc_round_trips_for_every_variant() {
            for &fmt in nokhwa_core::types::frame_formats() {
                let fcc = frameformat_to_fourcc(fmt);
                let back = fourcc_to_frameformat(fcc);
                assert_eq!(
                    back,
                    Some(fmt),
                    "{fmt:?} did not survive frameformat_to_fourcc → fourcc_to_frameformat"
                );
            }
        }

        /// `fourcc_to_frameformat` returns `None` for `FourCC` tokens that
        /// `FrameFormat` does not recognise. Pin a representative unknown
        /// (`H264`) so the swallow-as-`None` contract is explicit. The
        /// production callers (`new_shared_device`'s format enumerator,
        /// `compatible_formats`'s frame-format filter) rely on this — a
        /// future "just guess the closest" tweak would silently surface
        /// unsupported encodings to consumers.
        #[test]
        fn fourcc_to_frameformat_unknown_returns_none() {
            assert_eq!(fourcc_to_frameformat(FourCC::new(b"H264")), None);
            assert_eq!(fourcc_to_frameformat(FourCC::new(b"\0\0\0\0")), None);
        }

        /// The byte representation of `frameformat_to_fourcc` must equal
        /// `FrameFormat::to_fourcc`'s string bytes for every variant
        /// *except* `GRAY`. This is the invariant that downstream tools
        /// (`v4l2-ctl --list-formats`, log lines that print `FourCC`
        /// bytes directly) depend on. Pin it so the v4l shim cannot
        /// silently diverge from the canonical `FourCC` table in
        /// `nokhwa-core`.
        ///
        /// `GRAY` is the deliberate exception: the V4L2 kernel wire token
        /// is `GREY` (`V4L2_PIX_FMT_GREY`), so the shim emits `GREY` while
        /// nokhwa-core keeps `GRAY` as its canonical token.
        #[test]
        fn frameformat_to_fourcc_matches_core_to_fourcc_bytes() {
            for &fmt in nokhwa_core::types::frame_formats() {
                let v4l_bytes = frameformat_to_fourcc(fmt).repr;
                if fmt == FrameFormat::GRAY {
                    assert_eq!(
                        &v4l_bytes[..],
                        b"GREY",
                        "v4l grayscale fourcc must use the kernel wire token GREY"
                    );
                    continue;
                }
                let core_bytes = fmt.to_fourcc().as_bytes();
                assert_eq!(
                    &v4l_bytes[..],
                    core_bytes,
                    "v4l fourcc bytes for {fmt:?} diverge from FrameFormat::to_fourcc"
                );
            }
        }

        /// Grayscale must survive the V4L2 boundary even though the kernel
        /// wire token (`GREY`) differs from nokhwa-core's canonical `GRAY`.
        /// A grayscale camera enumerating `GREY` formats must map to
        /// `FrameFormat::GRAY`, and a `set_format(GRAY)` must submit `GREY`
        /// to the ioctl — otherwise grayscale devices are silently
        /// unusable.
        #[test]
        fn grayscale_translates_grey_kernel_token() {
            assert_eq!(
                fourcc_to_frameformat(FourCC::new(b"GREY")),
                Some(FrameFormat::GRAY),
                "kernel GREY token must map to FrameFormat::GRAY"
            );
            assert_eq!(
                &frameformat_to_fourcc(FrameFormat::GRAY).repr[..],
                b"GREY",
                "FrameFormat::GRAY must submit the kernel GREY token"
            );
        }

        /// Canonical V4L2 standard form: `numerator == 1` →
        /// `denominator` is the integer fps. Pin the happy path
        /// (`{1, 30}` → 30) so a future "round to nearest fps" tweak
        /// cannot silently smuggle in fractional rates that
        /// `CameraFormat`'s `u32` fps cannot honour.
        #[test]
        fn interval_to_fps_one_over_thirty_returns_thirty() {
            let interval = v4l::Fraction::new(1, 30);
            assert_eq!(interval_to_fps(interval).unwrap(), 30);
        }

        /// V4L2 reports `time-per-frame`, so `{1, 60}` → 60 fps.
        /// Pin a second whole-fps form to lock that the helper
        /// returns the *denominator*, not the numerator (the
        /// MSMF/AVF frame-rate helpers go the other way). A
        /// regression of this orientation would surface every
        /// camera as 1 fps.
        #[test]
        fn interval_to_fps_one_over_sixty_returns_sixty() {
            let interval = v4l::Fraction::new(1, 60);
            assert_eq!(interval_to_fps(interval).unwrap(), 60);
        }

        /// `{1001, 30000}` is the V4L2 form of NTSC's 29.97 fps.
        /// `CameraFormat` cannot represent fractional rates as
        /// `u32`, so the helper rejects rather than silently
        /// rounding to 30 fps. Mirrors the same policy
        /// `parse_frame_rate_fraction` enforces in MSMF and
        /// `fraction_to_fps` enforces in `GStreamer`.
        #[test]
        fn interval_to_fps_ntsc_fractional_form_errors() {
            let interval = v4l::Fraction::new(1001, 30_000);
            assert!(interval_to_fps(interval).is_err());
        }

        /// Unreduced whole-fps form `{2, 60}` (= 30 fps) is still
        /// rejected. V4L2 drivers normalise to numerator==1 in
        /// practice, and the previous inline shape carried a dead
        /// `{2, 60} → 30` branch that the upstream guard kept
        /// unreachable. Pin the rejection so a future relaxation
        /// is a deliberate API change rather than an accidental
        /// resurrection of the dead branch.
        #[test]
        fn interval_to_fps_unreduced_form_errors() {
            let interval = v4l::Fraction::new(2, 60);
            assert!(interval_to_fps(interval).is_err());
        }

        /// `numerator == 0` (degenerate driver output) must error
        /// rather than panicking via `denominator % 0` (which the
        /// previous inline shape's second clause would have done
        /// if the OR's left operand had not short-circuited).
        /// Helper hits the `numerator != 1` arm first, so no
        /// modulo-by-zero risk exists, but pin it explicitly.
        #[test]
        fn interval_to_fps_zero_numerator_errors() {
            let interval = v4l::Fraction::new(0, 30);
            assert!(interval_to_fps(interval).is_err());
        }

        /// Bug fix: `expand_frame_interval` with a Stepwise interval whose
        /// `step.numerator == 0` must not panic. `step_by(0)` on a range
        /// panics unconditionally. The guard returns `vec![]` for zero-step
        /// Stepwise, matching the analogous "denominator guard" above it.
        #[test]
        fn expand_frame_interval_stepwise_zero_step_does_not_panic() {
            let resolution = Resolution::new(640, 480);
            let interval = FrameIntervalEnum::Stepwise(Stepwise {
                min: Fraction::new(1, 30),
                max: Fraction::new(1, 60),
                step: Fraction::new(0, 1), // zero numerator → step_by(0) used to panic
            });
            // Must not panic; empty result is acceptable.
            let result = expand_frame_interval(interval, resolution, FrameFormat::MJPEG);
            assert!(result.is_empty());
        }

        /// `expand_stepwise_resolutions` must include the max endpoint.
        /// The old inline `..s.max_width` (exclusive) in `new()` missed it,
        /// causing "Failed to fulfill" when the caller requested the max
        /// resolution. The helper uses inclusive bounds; pin the contract.
        #[test]
        fn expand_stepwise_resolutions_includes_max_endpoint() {
            // Choose a max that is NOT a preset so the test does not pass by
            // accident because a preset happens to equal the max.
            let out = run(320, 1921, 1, 240, 1081, 1);
            assert!(
                out.contains(&Resolution::new(1921, 1081)),
                "max endpoint (1921, 1081) must be present; got {out:?}"
            );
        }

        /// `expand_stepwise_resolutions` with a zero step must not panic.
        /// Step == 0 means "continuous" in V4L2; the helper treats it as
        /// "always aligned", so the range should yield min + matching presets
        /// + max without panicking.
        #[test]
        fn expand_stepwise_resolutions_zero_step_does_not_panic() {
            // step 0 on both axes: should produce min, all fitting presets,
            // and max without panicking.
            let out = run(320, 1920, 0, 240, 1080, 0);
            // min and max must always be present
            assert!(out.contains(&Resolution::new(320, 240)));
            assert!(out.contains(&Resolution::new(1920, 1080)));
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod internal {
    use nokhwa_core::buffer::Buffer;
    use nokhwa_core::error::NokhwaError;
    use nokhwa_core::traits::{CameraDevice, FrameSource};
    use nokhwa_core::types::{
        ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
        FrameFormat, KnownCameraControl, RequestedFormat,
    };
    use std::borrow::Cow;

    /// Attempts to convert a [`KnownCameraControl`] into a V4L2 Control ID.
    /// If the associated control is not found, this will return `None` (`ColorEnable`, `Roll`)
    #[must_use]
    pub fn known_camera_control_to_id(_ctrl: KnownCameraControl) -> u32 {
        0
    }

    /// Attempts to convert a [`u32`] V4L2 Control ID into a [`KnownCameraControl`]
    /// If the associated control is not found, this will return `None` (`ColorEnable`, `Roll`)
    #[allow(clippy::cast_lossless)]
    #[must_use]
    pub fn id_to_known_camera_control(id: u32) -> KnownCameraControl {
        KnownCameraControl::Other(id as u128)
    }

    /// Non-Linux stub for `V4LCaptureDevice`.
    ///
    /// Exists so that cross-platform documentation builds and downstream
    /// code that merely references the type can compile on macOS / Windows
    /// hosts. Fallible methods return [`NokhwaError::NotImplementedError`];
    /// infallible methods panic via `unreachable!()` — they cannot be
    /// reached in practice because `V4LCaptureDevice::new` errors off
    /// Linux, so no value of this stub type can exist at runtime via
    /// the public constructor path.
    ///
    /// Mirrors the off-Windows stub used by `MediaFoundationCaptureDevice`.
    pub struct V4LCaptureDevice;

    /// Shared error for fallible stub methods.
    fn not_on_this_platform() -> NokhwaError {
        NokhwaError::NotImplementedError("V4L2 only on Linux".to_string())
    }

    /// Shared panic for infallible stub methods. These methods cannot
    /// return an error and should never be called in practice because
    /// `V4LCaptureDevice::new` errors off Linux, so no `V4LCaptureDevice`
    /// value can be produced through the public constructor path.
    #[cold]
    #[inline(never)]
    fn stub_unreachable() -> ! {
        unreachable!("V4L stub: only available on Linux")
    }

    #[allow(unused_variables)]
    impl V4LCaptureDevice {
        /// Creates a new capture device using the `V4L2` backend. Indexes are gives to devices by the OS, and usually numbered by order of discovery.
        /// # Errors
        /// This function will error if the camera is currently busy or if `V4L2` can't read device information.
        #[allow(clippy::too_many_lines)]
        pub fn new(index: &CameraIndex, cam_fmt: RequestedFormat) -> Result<Self, NokhwaError> {
            Err(not_on_this_platform())
        }

        /// Force refreshes the inner [`CameraFormat`] state.
        /// # Errors
        /// If the internal representation in the driver is invalid, this will error.
        pub fn force_refresh_camera_format(&mut self) -> Result<(), NokhwaError> {
            Err(not_on_this_platform())
        }
    }

    #[allow(unused_variables)]
    impl CameraDevice for V4LCaptureDevice {
        fn backend(&self) -> ApiBackend {
            ApiBackend::Video4Linux
        }

        fn info(&self) -> &CameraInfo {
            stub_unreachable()
        }

        fn controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
            Err(not_on_this_platform())
        }

        fn set_control(
            &mut self,
            id: KnownCameraControl,
            value: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            Err(not_on_this_platform())
        }
    }

    #[allow(unused_variables)]
    impl FrameSource for V4LCaptureDevice {
        fn negotiated_format(&self) -> CameraFormat {
            stub_unreachable()
        }

        fn set_format(&mut self, f: CameraFormat) -> Result<(), NokhwaError> {
            Err(not_on_this_platform())
        }

        fn compatible_formats(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
            Err(not_on_this_platform())
        }

        fn compatible_fourcc(&mut self) -> Result<Vec<FrameFormat>, NokhwaError> {
            Err(not_on_this_platform())
        }

        fn open(&mut self) -> Result<(), NokhwaError> {
            Err(not_on_this_platform())
        }

        fn is_open(&self) -> bool {
            false
        }

        fn frame(&mut self) -> Result<Buffer, NokhwaError> {
            Err(not_on_this_platform())
        }

        fn frame_raw(&mut self) -> Result<Cow<'_, [u8]>, NokhwaError> {
            Err(not_on_this_platform())
        }

        fn close(&mut self) -> Result<(), NokhwaError> {
            Err(not_on_this_platform())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            id_to_known_camera_control, known_camera_control_to_id, not_on_this_platform,
            V4LCaptureDevice,
        };
        use nokhwa_core::error::NokhwaError;
        use nokhwa_core::format_types::Mjpeg;
        use nokhwa_core::traits::{CameraDevice, FrameSource};
        use nokhwa_core::types::{
            ApiBackend, CameraFormat, CameraIndex, ControlValueSetter, FrameFormat,
            KnownCameraControl, RequestedFormat, RequestedFormatType, Resolution,
        };

        // Pin the contract that the off-Linux stub never hands out a live
        // device: every fallible method returns `NotImplementedError` and
        // `V4LCaptureDevice::new` errors deterministically. Mirrors the
        // off-Windows MSMF stub coverage. Without these tests a future
        // refactor could regress one of the methods to `panic!()` /
        // `todo!()` (the original bug fixed earlier) and the cross-
        // platform docs build would still pass.

        fn assert_not_implemented(err: &NokhwaError) {
            assert!(
                matches!(err, NokhwaError::NotImplementedError(_)),
                "expected NotImplementedError, got {err:?}",
            );
        }

        #[test]
        fn shared_error_helper_is_not_implemented() {
            assert_not_implemented(&not_on_this_platform());
        }

        #[test]
        fn new_errors_off_linux() {
            // `V4LCaptureDevice` intentionally does not implement `Debug`,
            // so we can't use `.expect_err`.
            match V4LCaptureDevice::new(
                &CameraIndex::Index(0),
                RequestedFormat::new::<Mjpeg>(RequestedFormatType::AbsoluteHighestFrameRate),
            ) {
                Err(err) => assert_not_implemented(&err),
                Ok(_) => panic!("stub `new` must always error off Linux"),
            }
        }

        #[test]
        fn force_refresh_camera_format_errors_off_linux() {
            // Same `Debug`-less pattern as `new_errors_off_linux`. We can
            // synthesize a `V4LCaptureDevice` directly because it's a unit
            // struct — the public `new` constructor errors, but the type
            // itself is constructable for tests pinning the trait surface.
            let mut dev = V4LCaptureDevice;
            match dev.force_refresh_camera_format() {
                Err(err) => assert_not_implemented(&err),
                Ok(()) => panic!("stub `force_refresh_camera_format` must error off Linux"),
            }
        }

        #[test]
        fn known_camera_control_id_helpers_are_no_ops() {
            // The off-Linux stubs collapse the V4L2 control-ID mapping to
            // a constant 0 / `Other(id)` round-trip. Pin that contract so
            // the cross-platform docs build can't silently drift.
            assert_eq!(
                known_camera_control_to_id(KnownCameraControl::Brightness),
                0
            );
            assert_eq!(known_camera_control_to_id(KnownCameraControl::Other(42)), 0);
            match id_to_known_camera_control(42) {
                KnownCameraControl::Other(id) => assert_eq!(id, 42),
                other => panic!("expected Other(42), got {other:?}"),
            }
        }

        #[test]
        fn backend_reports_video_for_linux() {
            let dev = V4LCaptureDevice;
            assert_eq!(dev.backend(), ApiBackend::Video4Linux);
        }

        #[test]
        fn camera_device_fallible_methods_return_not_implemented() {
            let mut dev = V4LCaptureDevice;
            assert_not_implemented(&dev.controls().expect_err("stub controls() must error"));
            let err = dev
                .set_control(
                    KnownCameraControl::Brightness,
                    ControlValueSetter::Integer(0),
                )
                .expect_err("stub set_control() must error");
            assert_not_implemented(&err);
        }

        #[test]
        fn frame_source_fallible_methods_return_not_implemented() {
            let mut dev = V4LCaptureDevice;
            assert_not_implemented(
                &dev.set_format(CameraFormat::new(
                    Resolution::new(640, 480),
                    FrameFormat::MJPEG,
                    30,
                ))
                .expect_err("stub set_format() must error"),
            );
            assert_not_implemented(
                &dev.compatible_formats()
                    .expect_err("stub compatible_formats() must error"),
            );
            assert_not_implemented(
                &dev.compatible_fourcc()
                    .expect_err("stub compatible_fourcc() must error"),
            );
            assert_not_implemented(&dev.open().expect_err("stub open() must error"));
            assert_not_implemented(&dev.frame().expect_err("stub frame() must error"));
            assert_not_implemented(&dev.frame_raw().expect_err("stub frame_raw() must error"));
            assert_not_implemented(&dev.close().expect_err("stub close() must error"));
        }

        #[test]
        fn is_open_reports_false() {
            let dev = V4LCaptureDevice;
            assert!(!dev.is_open());
        }

        #[test]
        #[should_panic(expected = "V4L stub: only available on Linux")]
        fn info_panics_via_stub_unreachable() {
            let dev = V4LCaptureDevice;
            let _info = dev.info();
        }

        #[test]
        #[should_panic(expected = "V4L stub: only available on Linux")]
        fn negotiated_format_panics_via_stub_unreachable() {
            let dev = V4LCaptureDevice;
            let _fmt = dev.negotiated_format();
        }
    }
}

pub use internal::*;

mod hotplug;
pub use hotplug::V4LHotplugContext;
