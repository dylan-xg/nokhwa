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
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::too_many_lines)]

//! # nokhwa-bindings-windows-msmf
//! This crate is the `MediaFoundation` bindings for the `nokhwa` crate.
//!
//! It is not meant for general consumption. If you are looking for a Windows camera capture crate, consider using `nokhwa` with feature `input-msmf`.
//!
//! No support or API stability will be given. Subject to change at any time.

#[cfg(all(windows, not(feature = "docs-only")))]
pub mod wmf {
    use nokhwa_core::error::NokhwaError;
    use nokhwa_core::types::{
        ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueDescription,
        ControlValueSetter, FrameFormat, KnownCameraControl, KnownCameraControlFlag, Resolution,
    };
    use std::ffi::c_void;
    use std::{
        borrow::Cow,
        cell::Cell,
        mem::{ManuallyDrop, MaybeUninit},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, LazyLock, Mutex,
        },
        time::Duration,
    };
    use windows::Win32::Media::DirectShow::CameraControl_Flags_Manual;
    use windows::Win32::Media::MediaFoundation::{
        MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
        MF_SOURCE_READERF_ERROR, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    };
    use windows::{
        core::{Interface, GUID, PWSTR},
        Win32::{
            Media::{
                DirectShow::{
                    CameraControl_Exposure, CameraControl_Focus, CameraControl_Iris,
                    CameraControl_Pan, CameraControl_Tilt, CameraControl_Zoom, IAMCameraControl,
                    IAMVideoProcAmp, VideoProcAmp_BacklightCompensation, VideoProcAmp_Brightness,
                    VideoProcAmp_ColorEnable, VideoProcAmp_Contrast, VideoProcAmp_Gain,
                    VideoProcAmp_Gamma, VideoProcAmp_Hue, VideoProcAmp_Saturation,
                    VideoProcAmp_Sharpness, VideoProcAmp_WhiteBalance,
                },
                KernelStreaming::GUID_NULL,
                MediaFoundation::{
                    IMFActivate, IMFAttributes, IMFMediaSource, IMFMediaType, IMFSample,
                    IMFSourceReader, MFCreateAttributes, MFCreateSourceReaderFromMediaSource,
                    MFEnumDeviceSources, MFShutdown, MFStartup, MFSTARTUP_NOSOCKET, MF_API_VERSION,
                    MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_FRAME_RATE,
                    MF_MT_FRAME_RATE_RANGE_MAX, MF_MT_FRAME_RATE_RANGE_MIN, MF_MT_FRAME_SIZE,
                    MF_MT_SUBTYPE, MF_READWRITE_DISABLE_CONVERTERS,
                },
            },
            System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT},
        },
    };

    static INITIALIZED: LazyLock<Arc<Mutex<bool>>> = LazyLock::new(|| Arc::new(Mutex::new(false)));
    static CAMERA_REFCNT: LazyLock<Arc<AtomicUsize>> =
        LazyLock::new(|| Arc::new(AtomicUsize::new(0)));

    // See: https://stackoverflow.com/questions/80160/what-does-coinit-speed-over-memory-do
    const CO_INIT_MULTITHREADED: COINIT = COINIT(0x0);
    const CO_INIT_DISABLE_OLE1DDE: COINIT = COINIT(0x4);

    // See: https://gix.github.io/media-types/#major-types
    const MF_VIDEO_FORMAT_YUY2: GUID = GUID::from_values(
        0x3259_5559,
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    );
    const MF_VIDEO_FORMAT_MJPEG: GUID = GUID::from_values(
        0x4750_4A4D,
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    );
    const MF_VIDEO_FORMAT_GRAY: GUID = GUID::from_values(
        0x3030_3859,
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    );
    const MF_VIDEO_FORMAT_NV12: GUID = GUID::from_values(
        0x3231_564E,
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    );
    const MF_VIDEO_FORMAT_RGB24: GUID = GUID::from_values(
        0x0000_0014,
        0x0000,
        0x0010,
        [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71],
    );

    const MF_SOURCE_READER_MEDIASOURCE: u32 = 0xFFFF_FFFF;

    /// Mirror of windows-rs's `MF_SOURCE_READER_FIRST_VIDEO_STREAM.0` cast to
    /// `u32` for the `IMFSourceReader` stream-index APIs. The original is an
    /// `i32` sentinel (`-4`); `as u32` reinterprets to `0xFFFF_FFFC`, which is
    /// the value Media Foundation expects. Kept as a single const so the
    /// `clippy::cast_sign_loss` suppression isn't sprinkled across the file.
    #[allow(clippy::cast_sign_loss)]
    const MF_FIRST_VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

    fn guid_to_frameformat(guid: GUID) -> Option<FrameFormat> {
        match guid {
            MF_VIDEO_FORMAT_NV12 => Some(FrameFormat::NV12),
            MF_VIDEO_FORMAT_RGB24 => Some(FrameFormat::RAWBGR),
            MF_VIDEO_FORMAT_GRAY => Some(FrameFormat::GRAY),
            MF_VIDEO_FORMAT_YUY2 => Some(FrameFormat::YUYV),
            MF_VIDEO_FORMAT_MJPEG => Some(FrameFormat::MJPEG),
            _ => None,
        }
    }

    struct ParsedMediaType {
        media_type: IMFMediaType,
        frame_format: FrameFormat,
        resolution: Resolution,
        frame_rates: Vec<u32>,
    }

    /// Decode an MSMF frame-rate `UINT64` attribute
    /// (`MF_MT_FRAME_RATE`, `..._RANGE_MIN`, `..._RANGE_MAX`) into a
    /// whole-frame integer fps, or `None` if the value cannot be
    /// expressed that way.
    ///
    /// Layout: top 32 bits = numerator, bottom 32 bits = denominator.
    /// We currently only consume rates whose denominator is exactly 1
    /// — anything else (e.g. `30000 / 1001` for NTSC's 29.97 fps) is
    /// dropped because the [`CameraFormat`] surface stores fps as a
    /// `u32`, and silently rounding a fractional rate would produce
    /// a value the device cannot honour. A nonzero whole-fps rate is
    /// returned as `Some(numerator)`.
    fn parse_frame_rate_fraction(fraction_u64: u64) -> Option<u32> {
        let numerator = (fraction_u64 >> 32) as u32;
        let denominator = fraction_u64 as u32;
        if denominator != 1 {
            return None;
        }
        if numerator == 0 {
            return None;
        }
        Some(numerator)
    }

    /// Extract the numerator from a packed `MF_MT_FRAME_RATE` `UINT64`
    /// (`numerator << 32 | denominator`) without the strict
    /// integer-only / nonzero validation [`parse_frame_rate_fraction`]
    /// applies during enumeration.
    ///
    /// Used by `format_refreshed` to decode the *negotiated* media
    /// type after a successful `set_camera_format`. By that point the
    /// source reader has already accepted the mode, so we trust the
    /// numerator and ignore the denominator (we cannot represent
    /// fractional rates in `CameraFormat`'s `u32` fps anyway —
    /// enumeration dropped such modes upstream, so an active mode
    /// reaching this helper is integer-fps in practice).
    ///
    /// Replaces an inline `fps as u32` cast that was reading the
    /// *denominator* (low 32 bits) instead of the numerator — the
    /// `device_format` cache silently held `frame_rate = 1` after
    /// every format change.
    fn frame_rate_numerator(fraction_u64: u64) -> u32 {
        (fraction_u64 >> 32) as u32
    }

    /// Decode a packed `MF_MT_FRAME_SIZE` `UINT64` into `(width,
    /// height)`. Microsoft packs the width in the *high* 32 bits and
    /// the height in the *low* 32 bits — the same shape as
    /// `MF_MT_FRAME_RATE`'s numerator/denominator layout.
    ///
    /// Centralised here so the two call sites (enumeration in
    /// `parse_native_media_types` and the cached-format refresh in
    /// `format_refreshed`) can not drift, and so the cast can be
    /// pinned without an `IMFMediaType` round-trip. Mirrors
    /// `frame_rate_numerator` in shape and intent.
    fn parse_frame_size(packed: u64) -> (u32, u32) {
        let width = (packed >> 32) as u32;
        let height = packed as u32;
        (width, height)
    }

    fn parse_native_media_types(
        source_reader: &IMFSourceReader,
    ) -> Result<Vec<ParsedMediaType>, NokhwaError> {
        let mut result = vec![];
        let mut index = 0;

        while let Ok(media_type) =
            unsafe { source_reader.GetNativeMediaType(MF_FIRST_VIDEO_STREAM, index) }
        {
            index += 1;

            let fourcc = match unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) } {
                Ok(fcc) => fcc,
                Err(why) => {
                    return Err(NokhwaError::get_property("MF_MT_SUBTYPE", why.to_string()))
                },
            };

            let Some(frame_format) = guid_to_frameformat(fourcc) else {
                continue;
            };

            let (width, height) = match unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) } {
                Ok(res_u64) => parse_frame_size(res_u64),
                Err(why) => {
                    return Err(NokhwaError::get_property(
                        "MF_MT_FRAME_SIZE",
                        why.to_string(),
                    ))
                },
            };

            let frame_rates = {
                let mut rates = Vec::with_capacity(3); // max, default, min
                for attr in [
                    &MF_MT_FRAME_RATE_RANGE_MAX,
                    &MF_MT_FRAME_RATE,
                    &MF_MT_FRAME_RATE_RANGE_MIN,
                ] {
                    if let Ok(fraction_u64) = unsafe { media_type.GetUINT64(attr) } {
                        if let Some(rate) = parse_frame_rate_fraction(fraction_u64) {
                            rates.push(rate);
                        }
                    }
                }
                rates
            };

            result.push(ParsedMediaType {
                media_type,
                frame_format,
                resolution: Resolution::new(width, height),
                frame_rates,
            });
        }

        Ok(result)
    }

    pub fn initialize_mf() -> Result<(), NokhwaError> {
        // The init flag is a Mutex, not an atomic, so that a second caller which
        // loses the race blocks until the winner has *finished* CoInitializeEx +
        // MFStartup — not merely started them. A bare atomic CAS only guarantees a
        // single startup; a lost-race caller would observe `true` and proceed to
        // call MF APIs while the winner is still inside MFStartup, which is UB
        // (MFStartup is not re-entrant and MF must be fully started before use).
        // Holding the lock across MFStartup establishes the happens-before edge.
        // On failure the flag stays `false`, so the next caller retries rather
        // than being wedged at `true` with no MF runtime behind it.
        let mut initialized = INITIALIZED
            .lock()
            .map_err(|why| NokhwaError::InitializeError {
                backend: ApiBackend::MediaFoundation,
                error: format!("initialization lock poisoned: {why}"),
            })?;
        if *initialized {
            return Ok(());
        }

        if let Err(why) =
            unsafe { CoInitializeEx(None, CO_INIT_MULTITHREADED | CO_INIT_DISABLE_OLE1DDE).ok() }
        {
            return Err(NokhwaError::InitializeError {
                backend: ApiBackend::MediaFoundation,
                error: why.to_string(),
            });
        }

        if let Err(why) = unsafe { MFStartup(MF_API_VERSION, MFSTARTUP_NOSOCKET) } {
            unsafe { CoUninitialize() };
            return Err(NokhwaError::InitializeError {
                backend: ApiBackend::MediaFoundation,
                error: why.to_string(),
            });
        }
        *initialized = true;
        Ok(())
    }

    pub fn de_initialize_mf() -> Result<(), NokhwaError> {
        // Mirror of initialize_mf: hold the init lock across MFShutdown +
        // CoUninitialize so teardown is serialized against any concurrent
        // initialize_mf. If already de-initialised, bail. If MFShutdown errors we
        // leave the flag at false anyway — the runtime is in an unrecoverable
        // state and we should not retry teardown.
        let mut initialized = INITIALIZED
            .lock()
            .map_err(|why| NokhwaError::ShutdownError {
                backend: ApiBackend::MediaFoundation,
                error: format!("initialization lock poisoned: {why}"),
            })?;
        if !*initialized {
            return Ok(());
        }
        *initialized = false;

        unsafe {
            if let Err(why) = MFShutdown() {
                CoUninitialize();
                return Err(NokhwaError::ShutdownError {
                    backend: ApiBackend::MediaFoundation,
                    error: why.to_string(),
                });
            }
            CoUninitialize();
        }
        Ok(())
    }

    fn query_activate_pointers() -> Result<Vec<IMFActivate>, NokhwaError> {
        initialize_mf()?;

        let mut attributes: Option<IMFAttributes> = None;
        if let Err(why) = unsafe { MFCreateAttributes(&raw mut attributes, 1) } {
            return Err(NokhwaError::get_property("IMFAttributes", why.to_string()));
        }

        let attributes = match attributes {
            Some(attr) => {
                if let Err(why) = unsafe {
                    attr.SetGUID(
                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
                    )
                } {
                    return Err(NokhwaError::set_property(
                        "GUID MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE",
                        "MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID",
                        why.to_string(),
                    ));
                }
                attr
            },
            None => {
                return Err(NokhwaError::set_property(
                    "GUID MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE",
                    "MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID",
                    "Call to IMFAttributes::SetGUID failed - IMFAttributes is None",
                ));
            },
        };

        let mut count: u32 = 0;
        let mut unused_mf_activate: MaybeUninit<*mut Option<IMFActivate>> = MaybeUninit::uninit();

        if let Err(why) = unsafe {
            MFEnumDeviceSources(&attributes, unused_mf_activate.as_mut_ptr(), &raw mut count)
        } {
            return Err(NokhwaError::structure(
                "MFEnumDeviceSources",
                why.to_string(),
            ));
        }

        // SAFETY: MFEnumDeviceSources succeeded, so it always writes a value into
        // the out-parameter (NULL when count == 0, a CoTaskMem-allocated array when
        // count > 0). We MUST CoTaskMemFree the array pointer on every non-error
        // path — that is the Win32 contract for MFEnumDeviceSources. Each element
        // is moved out via ptr::read so that the COM object's own Drop (Release)
        // fires exactly once via the owned IMFActivate pushed into `device_list`,
        // while the CoTaskMem block itself is freed as raw memory afterward.
        let array_ptr = unsafe { unused_mf_activate.assume_init() };

        let mut device_list = vec![];
        for i in 0..count as usize {
            // Move the Option<IMFActivate> out of the array slot without running
            // Drop on the slot itself — the CoTaskMem block is freed below as raw
            // bytes, so a Drop here would double-Release the COM reference.
            let slot: Option<IMFActivate> = unsafe { std::ptr::read(array_ptr.add(i)) };
            if let Some(activate) = slot {
                device_list.push(activate);
            }
        }
        // Free the CoTaskMem-allocated array itself (NULL is a no-op per the Win32
        // spec, so this is always safe). Individual IMFActivate objects are now
        // owned by `device_list` and will be Released via their Drop impls.
        unsafe { CoTaskMemFree(Some(array_ptr.cast::<c_void>())) };

        Ok(device_list)
    }

    /// Free a `PWSTR` allocated by `GetAllocatedString` (caller-owns-buffer
    /// contract). Null pointers are silently ignored — `CoTaskMemFree` accepts
    /// `NULL` and is a no-op, but we check explicitly to make the intent clear.
    ///
    /// # Safety
    /// `p` must be either null or a pointer previously returned by a Win32
    /// `GetAllocatedString` / `CoTaskMemAlloc` call.  Calling this more than
    /// once for the same pointer is undefined behaviour.
    unsafe fn free_pwstr(p: PWSTR) {
        if !p.is_null() {
            // SAFETY: caller guarantees `p` comes from GetAllocatedString.
            unsafe { CoTaskMemFree(Some(p.as_ptr().cast::<c_void>())) };
        }
    }

    fn activate_to_descriptors(
        index: CameraIndex,
        imf_activate: &IMFActivate,
    ) -> Result<CameraInfo, NokhwaError> {
        let mut pwstr_name = PWSTR(&mut 0_u16);
        let mut len_pwstrname = 0;

        if let Err(why) = unsafe {
            imf_activate.GetAllocatedString(
                &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME,
                &raw mut pwstr_name,
                &raw mut len_pwstrname,
            )
        } {
            return Err(NokhwaError::get_property(
                "MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME",
                why.to_string(),
            ));
        }

        if pwstr_name.is_null() {
            return Err(NokhwaError::get_property(
                "MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME",
                "Call to IMFActivate::GetAllocatedString failed - PWSTR is null",
            ));
        }

        // Convert and immediately free the name buffer so that a later error
        // path cannot skip its CoTaskMemFree.
        let name = unsafe {
            let result = pwstr_name
                .to_string()
                .map_err(|x| NokhwaError::structure("PWSTR/String - Name", x.to_string()));
            // SAFETY: GetAllocatedString succeeded and returned non-null.
            free_pwstr(pwstr_name);
            result?
        };

        let mut pwstr_symlink = PWSTR(&mut 0_u16);
        let mut len_pwstrsymlink = 0;

        if let Err(why) = unsafe {
            imf_activate.GetAllocatedString(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK,
                &raw mut pwstr_symlink,
                &raw mut len_pwstrsymlink,
            )
        } {
            // pwstr_name was already freed above; only pwstr_symlink is relevant here,
            // and GetAllocatedString failing means no buffer was allocated for it.
            return Err(NokhwaError::get_property(
                "MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK",
                why.to_string(),
            ));
        }

        if pwstr_symlink.is_null() {
            return Err(NokhwaError::get_property(
                "MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK",
                "Call to IMFActivate::GetAllocatedString failed - PWSTR is null",
            ));
        }

        // Convert and immediately free the symlink buffer.
        let symlink = unsafe {
            let result = pwstr_symlink
                .to_string()
                .map_err(|x| NokhwaError::structure("PWSTR/String - Symlink", x.to_string()));
            // SAFETY: GetAllocatedString succeeded and returned non-null.
            free_pwstr(pwstr_symlink);
            result?
        };

        Ok(CameraInfo::new(
            &name,
            "MediaFoundation Camera",
            &symlink,
            false,
            index,
        ))
    }

    pub fn query() -> Result<Vec<CameraInfo>, NokhwaError> {
        let mut device_list = vec![];

        for (index, activate_ptr) in query_activate_pointers()?.into_iter().enumerate() {
            device_list.push(activate_to_descriptors(
                CameraIndex::Index(index as u32),
                &activate_ptr,
            )?);
        }
        Ok(device_list)
    }

    #[derive(Copy, Clone, Debug, PartialOrd, PartialEq, Eq)]
    enum MFControlId {
        ProcAmpBoolean(i32),
        ProcAmpRange(i32),
        CCRange(i32),
    }

    #[allow(clippy::cast_sign_loss)]
    fn kcc_to_i32(kcc: KnownCameraControl) -> Option<MFControlId> {
        let control_id = match kcc {
            KnownCameraControl::Brightness => MFControlId::ProcAmpRange(VideoProcAmp_Brightness.0),
            KnownCameraControl::Contrast => MFControlId::ProcAmpRange(VideoProcAmp_Contrast.0),
            KnownCameraControl::Hue => MFControlId::ProcAmpRange(VideoProcAmp_Hue.0),
            KnownCameraControl::Saturation => MFControlId::ProcAmpRange(VideoProcAmp_Saturation.0),
            KnownCameraControl::Sharpness => MFControlId::ProcAmpRange(VideoProcAmp_Sharpness.0),
            KnownCameraControl::Gamma => MFControlId::ProcAmpRange(VideoProcAmp_Gamma.0),
            KnownCameraControl::WhiteBalance => {
                MFControlId::ProcAmpRange(VideoProcAmp_WhiteBalance.0)
            },
            KnownCameraControl::BacklightComp => {
                MFControlId::ProcAmpBoolean(VideoProcAmp_BacklightCompensation.0)
            },
            KnownCameraControl::Gain => MFControlId::ProcAmpRange(VideoProcAmp_Gain.0),
            KnownCameraControl::Pan => MFControlId::CCRange(CameraControl_Pan.0),
            KnownCameraControl::Tilt => MFControlId::CCRange(CameraControl_Tilt.0),
            KnownCameraControl::Zoom => MFControlId::CCRange(CameraControl_Zoom.0),
            KnownCameraControl::Exposure => MFControlId::CCRange(CameraControl_Exposure.0),
            KnownCameraControl::Iris => MFControlId::CCRange(CameraControl_Iris.0),
            KnownCameraControl::Focus => MFControlId::CCRange(CameraControl_Focus.0),
            KnownCameraControl::Other(o) => {
                if o == VideoProcAmp_ColorEnable.0 as u128 {
                    MFControlId::ProcAmpRange(o as i32)
                } else {
                    return None;
                }
            },
        };

        Some(control_id)
    }

    fn kcc_to_i32_or_err(control: KnownCameraControl) -> Result<MFControlId, NokhwaError> {
        kcc_to_i32(control).ok_or_else(|| {
            NokhwaError::set_property("CameraControl", control.to_string(), "Does not exist")
        })
    }

    pub struct MediaFoundationDevice {
        is_open: Cell<bool>,
        device_specifier: CameraInfo,
        device_format: CameraFormat,
        /// Wrapped in `ManuallyDrop` so [`Drop::drop`] can release this
        /// COM interface *before* `de_initialize_mf()` calls
        /// `MFShutdown()` + `CoUninitialize()`. Releasing an
        /// `IMFSourceReader` after the Media Foundation platform has been
        /// shut down faults (`STATUS_ACCESS_VIOLATION`), and struct
        /// fields are dropped *after* the `Drop::drop` body — so the
        /// teardown order has to be made explicit here.
        source_reader: ManuallyDrop<IMFSourceReader>,
        /// Wallclock instant captured when the stream was started.
        /// MF sample timestamps are relative to stream start, so
        /// `stream_epoch + sample_time` gives us an absolute wallclock.
        stream_epoch: Option<Duration>,
    }

    /// Raw outparams populated by a single `GetRange` + `Get` call pair.
    struct ProcAmpReadout {
        min: i32,
        max: i32,
        step: i32,
        default: i32,
        value: i32,
        flag: i32,
    }

    /// Call `IAMVideoProcAmp::GetRange` then `IAMVideoProcAmp::Get` and
    /// return all six outparams.  Returns `Err` immediately if either call
    /// fails, with the same error message shape used by `control()`.
    unsafe fn query_proc_amp(
        video_proc_amp: &IAMVideoProcAmp,
        id: i32,
        control_id: MFControlId,
        control: KnownCameraControl,
    ) -> Result<ProcAmpReadout, NokhwaError> {
        let mut min = 0i32;
        let mut max = 0i32;
        let mut step = 0i32;
        let mut default = 0i32;
        let mut value = 0i32;
        let mut flag = 0i32;
        if let Err(why) = video_proc_amp.GetRange(
            id,
            &raw mut min,
            &raw mut max,
            &raw mut step,
            &raw mut default,
            &raw mut flag,
        ) {
            return Err(NokhwaError::get_property(
                format!("{control_id:?}: {control} - Range"),
                why.to_string(),
            ));
        }
        if let Err(why) = video_proc_amp.Get(id, &raw mut value, &raw mut flag) {
            return Err(NokhwaError::get_property(
                format!("{control_id:?}: {control} - Value"),
                why.to_string(),
            ));
        }
        Ok(ProcAmpReadout {
            min,
            max,
            step,
            default,
            value,
            flag,
        })
    }

    /// Call `IAMCameraControl::GetRange` then `IAMCameraControl::Get` and
    /// return all six outparams.  Returns `Err` immediately if either call
    /// fails, with the same error message shape used by `control()`.
    unsafe fn query_camera_control(
        camera_control: &IAMCameraControl,
        id: i32,
        control_id: MFControlId,
        control: KnownCameraControl,
    ) -> Result<ProcAmpReadout, NokhwaError> {
        let mut min = 0i32;
        let mut max = 0i32;
        let mut step = 0i32;
        let mut default = 0i32;
        let mut value = 0i32;
        let mut flag = 0i32;
        if let Err(why) = camera_control.GetRange(
            id,
            &raw mut min,
            &raw mut max,
            &raw mut step,
            &raw mut default,
            &raw mut flag,
        ) {
            return Err(NokhwaError::get_property(
                format!("{control_id:?}: {control} - Range"),
                why.to_string(),
            ));
        }
        if let Err(why) = camera_control.Get(id, &raw mut value, &raw mut flag) {
            return Err(NokhwaError::get_property(
                format!("{control_id:?}: {control} - Value"),
                why.to_string(),
            ));
        }
        Ok(ProcAmpReadout {
            min,
            max,
            step,
            default,
            value,
            flag,
        })
    }

    impl MediaFoundationDevice {
        pub fn new(index: CameraIndex) -> Result<Self, NokhwaError> {
            initialize_mf()?;
            match index {
                CameraIndex::Index(i) => {
                    let (media_source, device_descriptor) =
                        match query_activate_pointers()?.into_iter().nth(i as usize) {
                            Some(activate) => {
                                match unsafe { activate.ActivateObject::<IMFMediaSource>() } {
                                    Ok(media_source) => {
                                        (media_source, activate_to_descriptors(index, &activate)?)
                                    },
                                    Err(why) => {
                                        return Err(NokhwaError::open_device(
                                            index.to_string(),
                                            why.to_string(),
                                        ))
                                    },
                                }
                            },
                            None => {
                                return Err(NokhwaError::open_device(
                                    index.to_string(),
                                    "device not found",
                                ))
                            },
                        };

                    let source_reader_attr = {
                        let mut attr_opt: Option<IMFAttributes> = None;
                        if let Err(why) = unsafe { MFCreateAttributes(&raw mut attr_opt, 3) } {
                            return Err(NokhwaError::structure(
                                "MFCreateAttributes",
                                why.to_string(),
                            ));
                        }
                        let Some(attr) = attr_opt else {
                            return Err(NokhwaError::structure(
                                "MFCreateAttributes",
                                "Attributee Alloc Failure",
                            ));
                        };

                        if let Err(why) = unsafe {
                            attr.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, u32::from(true))
                        } {
                            return Err(NokhwaError::set_property(
                                "MF_READWRITE_DISABLE_CONVERTERS",
                                u32::from(true).to_string(),
                                why.to_string(),
                            ));
                        }

                        attr
                    };

                    let source_reader = match unsafe {
                        MFCreateSourceReaderFromMediaSource(&media_source, &source_reader_attr)
                    } {
                        Ok(sr) => sr,
                        Err(why) => {
                            return Err(NokhwaError::structure(
                                "MFCreateSourceReaderFromMediaSource",
                                why.to_string(),
                            ))
                        },
                    };

                    // increment refcnt (fetch_add is an atomic RMW; no load+store race)
                    CAMERA_REFCNT.fetch_add(1, Ordering::SeqCst);

                    Ok(MediaFoundationDevice {
                        is_open: Cell::new(false),
                        device_specifier: device_descriptor,
                        device_format: CameraFormat::default(),
                        source_reader: ManuallyDrop::new(source_reader),
                        stream_epoch: None,
                    })
                },
                CameraIndex::String(s) => {
                    // A pure-numeric string is a positional index, not a
                    // symbolic link — `open(CameraIndex::String("0"))`
                    // must reach the same device as
                    // `open(CameraIndex::Index(0))`. (The session-layer
                    // routes URL-like strings to GStreamer before they
                    // get here, so anything that reaches this arm is
                    // either a number or an MSMF symbolic-link path.)
                    if let Ok(index) = s.parse::<u32>() {
                        return Self::new(CameraIndex::Index(index));
                    }

                    let devicelist = query()?;
                    let mut id_eq = None;

                    for mfdev in devicelist {
                        if mfdev.misc() == s {
                            id_eq = Some(mfdev.index().as_index()?);
                            break;
                        }
                    }

                    match id_eq {
                        Some(index) => Self::new(CameraIndex::Index(index)),
                        None => Err(NokhwaError::open_device(s, "device not found")),
                    }
                },
            }
        }

        pub fn index(&self) -> &CameraIndex {
            self.device_specifier.index()
        }

        pub fn name(&self) -> String {
            self.device_specifier.human_name()
        }

        pub fn symlink(&self) -> String {
            self.device_specifier.misc()
        }

        fn get_camera_control_services(
            &self,
        ) -> Result<(IAMCameraControl, IAMVideoProcAmp), NokhwaError> {
            let camera_control = unsafe {
                let mut receiver: MaybeUninit<IAMCameraControl> = MaybeUninit::uninit();
                let ptr_receiver = receiver.as_mut_ptr();
                if let Err(why) = self.source_reader.GetServiceForStream(
                    MF_SOURCE_READER_MEDIASOURCE,
                    &GUID_NULL,
                    &IAMCameraControl::IID,
                    ptr_receiver
                        .cast::<IAMCameraControl>()
                        .cast::<*mut c_void>(),
                ) {
                    return Err(NokhwaError::set_property(
                        "MF_SOURCE_READER_MEDIASOURCE",
                        "IAMCameraControl",
                        why.to_string(),
                    ));
                }
                receiver.assume_init()
            };
            let video_proc_amp = unsafe {
                let mut receiver: MaybeUninit<IAMVideoProcAmp> = MaybeUninit::uninit();
                let ptr_receiver = receiver.as_mut_ptr();
                if let Err(why) = self.source_reader.GetServiceForStream(
                    MF_SOURCE_READER_MEDIASOURCE,
                    &GUID_NULL,
                    &IAMVideoProcAmp::IID,
                    ptr_receiver.cast::<IAMVideoProcAmp>().cast::<*mut c_void>(),
                ) {
                    return Err(NokhwaError::set_property(
                        "MF_SOURCE_READER_MEDIASOURCE",
                        "IAMVideoProcAmp",
                        why.to_string(),
                    ));
                }
                receiver.assume_init()
            };
            Ok((camera_control, video_proc_amp))
        }

        pub fn compatible_format_list(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
            let mut camera_format_list = vec![];
            for parsed in parse_native_media_types(&self.source_reader)? {
                for frame_rate in &parsed.frame_rates {
                    camera_format_list.push(CameraFormat::new(
                        parsed.resolution,
                        parsed.frame_format,
                        *frame_rate,
                    ));
                }
            }
            // MSMF advertises one native media type per discrete frame
            // rate, and each carries `MF_MT_FRAME_RATE_RANGE_MAX` /
            // `MF_MT_FRAME_RATE` / `MF_MT_FRAME_RATE_RANGE_MIN` — for a
            // discrete rate all three hold the same value, so a camera
            // exposing N discrete rates yields ~3N `(res, fmt, fps)`
            // tuples of which only N are distinct (the MX Brio reports
            // every YUYV mode 3×). Some drivers also list the same
            // mode under more than one media type. Collapse to the
            // canonical sorted + deduped shape, matching
            // `compatible_fourcc`'s `collect → sort → dedup`.
            camera_format_list.sort_unstable();
            camera_format_list.dedup();
            Ok(camera_format_list)
        }

        pub fn control(&self, control: KnownCameraControl) -> Result<CameraControl, NokhwaError> {
            let (camera_control, video_proc_amp) = self.get_camera_control_services()?;

            let control_id = kcc_to_i32_or_err(control)?;

            let (ctrl_value_set, flag) = match control_id {
                MFControlId::ProcAmpBoolean(id) => {
                    let r = unsafe { query_proc_amp(&video_proc_amp, id, control_id, control)? };
                    let desc = ControlValueDescription::Boolean {
                        value: r.value != 0,
                        default: r.default != 0,
                    };
                    (desc, r.flag)
                },
                MFControlId::ProcAmpRange(id) => {
                    let r = unsafe { query_proc_amp(&video_proc_amp, id, control_id, control)? };
                    let desc = ControlValueDescription::IntegerRange {
                        min: i64::from(r.min),
                        max: i64::from(r.max),
                        value: i64::from(r.value),
                        step: i64::from(r.step),
                        default: i64::from(r.default),
                    };
                    (desc, r.flag)
                },
                MFControlId::CCRange(id) => {
                    let r =
                        unsafe { query_camera_control(&camera_control, id, control_id, control)? };
                    let desc = ControlValueDescription::IntegerRange {
                        min: i64::from(r.min),
                        max: i64::from(r.max),
                        value: i64::from(r.value),
                        step: i64::from(r.step),
                        default: i64::from(r.default),
                    };
                    (desc, r.flag)
                },
            };

            let is_manual = if flag == CameraControl_Flags_Manual.0 {
                KnownCameraControlFlag::Manual
            } else {
                KnownCameraControlFlag::Automatic
            };

            Ok(CameraControl::new(
                control,
                control.to_string(),
                ctrl_value_set,
                vec![is_manual],
                true,
            ))
        }

        pub fn set_control(
            &mut self,
            control: KnownCameraControl,
            value: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            let (camera_control, video_proc_amp) = self.get_camera_control_services()?;

            let control_id = kcc_to_i32_or_err(control)?;

            let ctrl_value = match value {
                ControlValueSetter::Integer(i) => i as i32,
                ControlValueSetter::Boolean(b) => i32::from(b),
                v => {
                    return Err(NokhwaError::structure(
                        format!("ControlValueSetter {v}"),
                        "invalid value type",
                    ))
                },
            };

            // Writing an explicit value always means manual mode.  Using the
            // device's current auto/manual flag here caused the driver to
            // silently ignore the value when the device was in Auto mode,
            // making Auto→Manual transitions impossible via set_control.
            let flag = CameraControl_Flags_Manual;

            match control_id {
                MFControlId::ProcAmpBoolean(id) | MFControlId::ProcAmpRange(id) => unsafe {
                    if let Err(why) = video_proc_amp.Set(id, ctrl_value, flag.0) {
                        return Err(NokhwaError::set_property(
                            control.to_string(),
                            ctrl_value.to_string(),
                            why.to_string(),
                        ));
                    }
                },
                MFControlId::CCRange(id) => unsafe {
                    if let Err(why) = camera_control.Set(id, ctrl_value, flag.0) {
                        return Err(NokhwaError::set_property(
                            control.to_string(),
                            ctrl_value.to_string(),
                            why.to_string(),
                        ));
                    }
                },
            }

            Ok(())
        }

        #[allow(clippy::cast_sign_loss)]
        pub fn format_refreshed(&mut self) -> Result<CameraFormat, NokhwaError> {
            match unsafe {
                self.source_reader
                    .GetCurrentMediaType(MF_FIRST_VIDEO_STREAM)
            } {
                Ok(media_type) => {
                    let resolution = match unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) } {
                        Ok(res) => {
                            let (width, height) = parse_frame_size(res);
                            Resolution {
                                width_x: width,
                                height_y: height,
                            }
                        },
                        Err(why) => {
                            return Err(NokhwaError::get_property(
                                "MF_MT_FRAME_SIZE",
                                why.to_string(),
                            ))
                        },
                    };

                    let frame_rate = match unsafe { media_type.GetUINT64(&MF_MT_FRAME_RATE) } {
                        Ok(fps) => frame_rate_numerator(fps),
                        Err(why) => {
                            return Err(NokhwaError::get_property(
                                "MF_MT_FRAME_RATE",
                                why.to_string(),
                            ))
                        },
                    };

                    let format = match unsafe { media_type.GetGUID(&MF_MT_SUBTYPE) } {
                        Ok(fcc) => match guid_to_frameformat(fcc) {
                            Some(ff) => ff,
                            None => {
                                return Err(NokhwaError::get_property("MF_MT_SUBTYPE", "Unknown"))
                            },
                        },
                        Err(why) => {
                            return Err(NokhwaError::get_property("MF_MT_SUBTYPE", why.to_string()))
                        },
                    };

                    let cfmt = CameraFormat::new(resolution, format, frame_rate);
                    self.device_format = cfmt;

                    Ok(cfmt)
                },
                Err(why) => Err(NokhwaError::get_property(
                    "MF_SOURCE_READER_FIRST_VIDEO_STREAM",
                    why.to_string(),
                )),
            }
        }

        pub fn format(&self) -> CameraFormat {
            self.device_format
        }

        pub fn set_format(&mut self, format: CameraFormat) -> Result<(), NokhwaError> {
            // We need to make sure to use all the original attributes of the IMFMediaType to avoid problems.
            // Otherwise, constructing IMFMediaType from scratch can sometimes fail due to not exactly matching.
            // Therefore, we search for the first media_type that matches and also works correctly.

            let mut last_error: Option<NokhwaError> = None;

            for parsed in parse_native_media_types(&self.source_reader)? {
                if parsed.frame_format != format.format() {
                    continue;
                }
                if parsed.resolution != format.resolution() {
                    continue;
                }

                for frame_rate in &parsed.frame_rates {
                    if *frame_rate == format.frame_rate() {
                        let result = unsafe {
                            self.source_reader.SetCurrentMediaType(
                                MF_FIRST_VIDEO_STREAM,
                                None,
                                &parsed.media_type,
                            )
                        };

                        match result {
                            Ok(()) => {
                                // `format_refreshed` reads the actually-negotiated
                                // media type back from the reader and caches it,
                                // so it is the single source of truth for
                                // `device_format` — don't pre-write the requested
                                // (unconfirmed) value here.
                                self.format_refreshed()?;
                                return Ok(());
                            },
                            Err(why) => {
                                last_error = Some(NokhwaError::set_property(
                                    "MF_SOURCE_READER_FIRST_VIDEO_STREAM",
                                    format!("{:?}", parsed.media_type),
                                    why.to_string(),
                                ));
                            },
                        }
                    }
                }
            }

            if let Some(err) = last_error {
                return Err(err);
            }

            Err(NokhwaError::InitializeError {
                backend: ApiBackend::MediaFoundation,
                error: "Failed to fulfill requested format".to_string(),
            })
        }

        pub fn is_stream_open(&self) -> bool {
            self.is_open.get()
        }

        pub fn start_stream(&mut self) -> Result<(), NokhwaError> {
            if let Err(why) = unsafe {
                self.source_reader
                    .SetStreamSelection(MF_FIRST_VIDEO_STREAM, true)
            } {
                return Err(NokhwaError::OpenStreamError {
                    message: why.to_string(),
                    backend: Some(ApiBackend::MediaFoundation),
                });
            }

            self.stream_epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok();
            self.is_open.set(true);
            Ok(())
        }

        pub fn raw_bytes(&mut self) -> Result<(Cow<'_, [u8]>, Option<Duration>), NokhwaError> {
            let frame_fmt = Some(self.device_format.format());
            // ReadSample populates this outparam directly; pre-allocating an
            // IMFSample with MFCreateSample() was wasted work — the driver COM-
            // releases the pre-created object and replaces it with the captured one.
            let mut imf_sample: Option<IMFSample> = None;
            let mut stream_flags = 0;
            let mut sample_time_100ns: i64 = 0;
            {
                loop {
                    if let Err(why) = unsafe {
                        self.source_reader.ReadSample(
                            MF_FIRST_VIDEO_STREAM,
                            0,
                            None,
                            Some(&raw mut stream_flags),
                            Some(&raw mut sample_time_100ns),
                            Some(&raw mut imf_sample),
                        )
                    } {
                        return Err(NokhwaError::ReadFrameError {
                            message: why.to_string(),
                            format: frame_fmt,
                        });
                    }

                    // Guard against infinite spin when the stream ends or
                    // errors (ReadSample returns Ok but leaves sample=None).
                    if (stream_flags
                        & (MF_SOURCE_READERF_ERROR.0 as u32
                            | MF_SOURCE_READERF_ENDOFSTREAM.0 as u32))
                        != 0
                    {
                        return Err(NokhwaError::ReadFrameError {
                            message: "stream ended or errored".to_string(),
                            format: frame_fmt,
                        });
                    }

                    // The driver can spontaneously renegotiate the media type
                    // mid-stream (resolution/fps/subtype). When it signals this,
                    // the accompanying sample is already in the new format, so
                    // refresh the cached `device_format` before returning — the
                    // caller tags the `Buffer` from `device_format`, and a stale
                    // tag would mislabel the frame's resolution/format.
                    if (stream_flags & (MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32)) != 0 {
                        self.format_refreshed()?;
                    }

                    if imf_sample.is_some() {
                        break;
                    }
                }
            }

            let Some(imf_sample) = imf_sample else {
                // shouldn't happen
                return Err(NokhwaError::ReadFrameError {
                    message: "No sample".to_string(),
                    format: frame_fmt,
                });
            };

            // Calculate absolute capture timestamp. A presentation time of 0 is a
            // valid first-frame stamp, so accept >= 0; a negative time (reported on
            // some format changes/seeks) has no meaningful wallclock mapping and
            // yields None rather than a fabricated stamp.
            let capture_ts = u64::try_from(sample_time_100ns)
                .ok()
                .and_then(|ticks_100ns| ticks_100ns.checked_mul(100))
                .map(Duration::from_nanos)
                .and_then(|sample_offset| {
                    self.stream_epoch
                        .and_then(|epoch| epoch.checked_add(sample_offset))
                });

            let buffer = match unsafe { imf_sample.ConvertToContiguousBuffer() } {
                Ok(buf) => buf,
                Err(why) => {
                    return Err(NokhwaError::ReadFrameError {
                        message: why.to_string(),
                        format: frame_fmt,
                    })
                },
            };

            let mut buffer_valid_length = 0;
            let mut buffer_start_ptr = std::ptr::null_mut::<u8>();

            if let Err(why) = unsafe {
                buffer.Lock(
                    &raw mut buffer_start_ptr,
                    None,
                    Some(&raw mut buffer_valid_length),
                )
            } {
                return Err(NokhwaError::ReadFrameError {
                    message: why.to_string(),
                    format: frame_fmt,
                });
            }

            if buffer_start_ptr.is_null() {
                let _ = unsafe { buffer.Unlock() };
                return Err(NokhwaError::ReadFrameError {
                    message: "Buffer Pointer Null".to_string(),
                    format: frame_fmt,
                });
            }

            if buffer_valid_length == 0 {
                let _ = unsafe { buffer.Unlock() };
                return Err(NokhwaError::ReadFrameError {
                    message: "Buffer Size is 0".to_string(),
                    format: frame_fmt,
                });
            }

            let mut data_slice = Vec::with_capacity(buffer_valid_length as usize);

            unsafe {
                // Copy pointer because we're bout to drop IMFSample
                data_slice.extend_from_slice(std::slice::from_raw_parts_mut(
                    buffer_start_ptr,
                    buffer_valid_length as usize,
                ) as &[u8]);
                // Every successful Lock must be paired with Unlock.
                let _ = buffer.Unlock();
            }

            Ok((Cow::from(data_slice), capture_ts))
        }

        pub fn stop_stream(&mut self) {
            self.stream_epoch = None;
            self.is_open.set(false);
        }
    }

    impl Drop for MediaFoundationDevice {
        fn drop(&mut self) {
            // swallow errors
            unsafe {
                let _ = self.source_reader.Flush(MF_FIRST_VIDEO_STREAM);

                // Release the IMFSourceReader *before* tearing down the
                // Media Foundation platform: an MF COM interface whose
                // last reference is dropped after `MFShutdown()` faults
                // (`STATUS_ACCESS_VIOLATION`). Struct fields drop after
                // this body, so the ordering has to be forced by hand.
                ManuallyDrop::drop(&mut self.source_reader);

                // decrement refcnt: fetch_sub returns the *previous* value, so == 1
                // means the count just reached 0 → tear down Media Foundation.
                // Note: underflow (drop called more times than new) would be a
                // separate logic bug; wrapping subtraction is intentional here and
                // de_initialize_mf() would not be called for any count != 1.
                if CAMERA_REFCNT.fetch_sub(1, Ordering::SeqCst) == 1 {
                    de_initialize_mf().ok();
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn guid_nv12() {
            assert_eq!(
                guid_to_frameformat(MF_VIDEO_FORMAT_NV12),
                Some(FrameFormat::NV12)
            );
        }

        #[test]
        fn guid_rgb24_maps_to_rawbgr() {
            assert_eq!(
                guid_to_frameformat(MF_VIDEO_FORMAT_RGB24),
                Some(FrameFormat::RAWBGR)
            );
        }

        #[test]
        fn guid_gray() {
            assert_eq!(
                guid_to_frameformat(MF_VIDEO_FORMAT_GRAY),
                Some(FrameFormat::GRAY)
            );
        }

        #[test]
        fn guid_yuy2() {
            assert_eq!(
                guid_to_frameformat(MF_VIDEO_FORMAT_YUY2),
                Some(FrameFormat::YUYV)
            );
        }

        #[test]
        fn guid_mjpeg() {
            assert_eq!(
                guid_to_frameformat(MF_VIDEO_FORMAT_MJPEG),
                Some(FrameFormat::MJPEG)
            );
        }

        #[test]
        fn guid_unknown_returns_none() {
            let unknown = GUID::from_values(0, 0, 0, [0; 8]);
            assert_eq!(guid_to_frameformat(unknown), None);
        }

        // Pin every KnownCameraControl -> MFControlId mapping. The variant
        // (ProcAmpRange / ProcAmpBoolean / CCRange) determines which
        // IAMVideoProcAmp / IAMCameraControl Get/Set routine `control()` and
        // `set_control` end up using, and — for the *Range variants — whether
        // the reported descriptor keeps its min/max bounds. A ranged control
        // (Pan/Exposure/Focus/…) that drifts off CCRange would surface a
        // descriptor with no limits even though IAMCameraControl::GetRange
        // returns them. The exact i32 IDs come from windows-rs constants — we
        // round-trip them so a future windows-rs major bump that renumbers
        // these would be caught here rather than at runtime on a customer's box.
        #[test]
        fn kcc_to_i32_maps_every_standard_control() {
            let expected: &[(KnownCameraControl, MFControlId)] = &[
                (
                    KnownCameraControl::Brightness,
                    MFControlId::ProcAmpRange(VideoProcAmp_Brightness.0),
                ),
                (
                    KnownCameraControl::Contrast,
                    MFControlId::ProcAmpRange(VideoProcAmp_Contrast.0),
                ),
                (
                    KnownCameraControl::Hue,
                    MFControlId::ProcAmpRange(VideoProcAmp_Hue.0),
                ),
                (
                    KnownCameraControl::Saturation,
                    MFControlId::ProcAmpRange(VideoProcAmp_Saturation.0),
                ),
                (
                    KnownCameraControl::Sharpness,
                    MFControlId::ProcAmpRange(VideoProcAmp_Sharpness.0),
                ),
                (
                    KnownCameraControl::Gamma,
                    MFControlId::ProcAmpRange(VideoProcAmp_Gamma.0),
                ),
                (
                    KnownCameraControl::WhiteBalance,
                    MFControlId::ProcAmpRange(VideoProcAmp_WhiteBalance.0),
                ),
                (
                    KnownCameraControl::BacklightComp,
                    MFControlId::ProcAmpBoolean(VideoProcAmp_BacklightCompensation.0),
                ),
                (
                    KnownCameraControl::Gain,
                    MFControlId::ProcAmpRange(VideoProcAmp_Gain.0),
                ),
                (
                    KnownCameraControl::Pan,
                    MFControlId::CCRange(CameraControl_Pan.0),
                ),
                (
                    KnownCameraControl::Tilt,
                    MFControlId::CCRange(CameraControl_Tilt.0),
                ),
                (
                    KnownCameraControl::Zoom,
                    MFControlId::CCRange(CameraControl_Zoom.0),
                ),
                (
                    KnownCameraControl::Exposure,
                    MFControlId::CCRange(CameraControl_Exposure.0),
                ),
                (
                    KnownCameraControl::Iris,
                    MFControlId::CCRange(CameraControl_Iris.0),
                ),
                (
                    KnownCameraControl::Focus,
                    MFControlId::CCRange(CameraControl_Focus.0),
                ),
            ];
            for (kcc, want) in expected {
                let got = kcc_to_i32(*kcc).expect("standard control must map");
                assert_eq!(got, *want, "wrong MFControlId for {kcc:?}");
            }
        }

        #[test]
        fn kcc_to_i32_other_color_enable_is_recognised() {
            // ColorEnable is the only `Other(_)` value the MSMF backend
            // accepts — it's a `VideoProcAmp_ColorEnable` boolean exposed
            // to nokhwa as `Other(VideoProcAmp_ColorEnable.0 as u128)`.
            // Pin that round-trip so the magic-number lookup doesn't
            // silently regress.
            let other = KnownCameraControl::Other(VideoProcAmp_ColorEnable.0 as u128);
            assert_eq!(
                kcc_to_i32(other),
                Some(MFControlId::ProcAmpRange(VideoProcAmp_ColorEnable.0)),
            );
        }

        #[test]
        fn kcc_to_i32_unknown_other_returns_none() {
            // A `KnownCameraControl::Other(_)` whose payload doesn't match
            // ColorEnable must fall through to `None` so `set_control` /
            // `control` can report `UnsupportedOperationError`.
            let other = KnownCameraControl::Other(0xDEAD_BEEF);
            assert_eq!(kcc_to_i32(other), None);
        }

        // `parse_frame_rate_fraction` decodes MSMF's `UINT64`
        // numerator/denominator layout. The contract: only whole-fps
        // rates with denominator == 1 survive; fractional rates
        // (denominator != 1, e.g. NTSC's 30000/1001) are dropped
        // because `CameraFormat` stores fps as `u32` and rounding
        // produces a value the device cannot honour. Pin all four
        // branches so a future "should we accept N/M for some N/M?"
        // refactor is caught here rather than at runtime.

        /// Numerator in the top 32 bits, denominator == 1 in the
        /// bottom 32 bits, both nonzero → `Some(numerator)`.
        #[test]
        fn parse_frame_rate_fraction_30_over_1_returns_30() {
            let fraction = (30_u64 << 32) | 1_u64;
            assert_eq!(parse_frame_rate_fraction(fraction), Some(30));
        }

        /// Denominator != 1 → `None` regardless of numerator. NTSC's
        /// 30000/1001 (29.97 fps) is the canonical real-world case.
        #[test]
        fn parse_frame_rate_fraction_30000_over_1001_returns_none() {
            let fraction = (30_000_u64 << 32) | 0x3E9_u64; // 1001
            assert_eq!(parse_frame_rate_fraction(fraction), None);
        }

        /// Numerator == 0 (denominator == 1) → `None`. A legitimate
        /// "no rate advertised" sentinel from the device.
        #[test]
        fn parse_frame_rate_fraction_zero_numerator_returns_none() {
            let fraction: u64 = 1;
            assert_eq!(parse_frame_rate_fraction(fraction), None);
        }

        /// Denominator == 0 → `None` (no division-by-zero risk: we
        /// short-circuit on `denominator != 1` first, but zero is
        /// still a `denominator != 1` value).
        #[test]
        fn parse_frame_rate_fraction_zero_denominator_returns_none() {
            let fraction: u64 = 30_u64 << 32;
            assert_eq!(parse_frame_rate_fraction(fraction), None);
        }

        /// Numerator at the upper edge of `u32` (`0xFFFF_FFFF`) with
        /// denominator == 1 round-trips intact. Pins that the
        /// `(u64 >> 32) as u32` cast in the helper does not lose the
        /// high bit.
        #[test]
        fn parse_frame_rate_fraction_max_numerator_round_trips() {
            let fraction = (u64::from(u32::MAX) << 32) | 1_u64;
            assert_eq!(parse_frame_rate_fraction(fraction), Some(u32::MAX));
        }

        /// Regression for `format_refreshed` silently reading the
        /// denominator instead of the numerator. The previous shape
        /// `let frame_rate = fps as u32` cast a `u64` to `u32` which
        /// keeps the *low* 32 bits — i.e. the denominator (almost
        /// always `1`). After every `set_camera_format` the cached
        /// `device_format.frame_rate` was therefore `1`, regardless
        /// of what fps the user had actually requested. The fix
        /// routes through `frame_rate_numerator`, which returns the
        /// *high* 32 bits.
        ///
        /// `30 << 32 | 1` is the canonical "30 fps integer-rate"
        /// MSMF fraction. The buggy shape returned `1`; the helper
        /// returns `30`.
        #[test]
        fn frame_rate_numerator_returns_high_word_not_denominator() {
            let fraction: u64 = (30_u64 << 32) | 1_u64;
            // Buggy shape (kept here as a witness, not used):
            assert_eq!(fraction as u32, 1, "the buggy `as u32` cast read 1");
            // Helper returns the numerator we actually want.
            assert_eq!(frame_rate_numerator(fraction), 30);
        }

        /// `frame_rate_numerator` is intentionally *less* strict than
        /// `parse_frame_rate_fraction`: it is called on the
        /// negotiated media type after the source reader has
        /// accepted the mode, so denominator validation is the
        /// caller's responsibility. Pin that a `denominator != 1`
        /// fraction (e.g. NTSC's `30000 / 1001`) still surfaces the
        /// numerator. In practice enumeration drops such modes
        /// upstream, but if Apple/Microsoft ever accepts one we
        /// surface the fps the device negotiated rather than `0` or
        /// the denominator.
        #[test]
        fn frame_rate_numerator_ignores_denominator() {
            let fraction: u64 = (30_000_u64 << 32) | 0x3E9_u64; // 1001
            assert_eq!(frame_rate_numerator(fraction), 30_000);
        }

        /// Both halves at `u32::MAX` round-trip — the helper must
        /// not lose the high bit on the `(u64 >> 32) as u32`
        /// cast, the same invariant we pin on
        /// `parse_frame_rate_fraction`'s numerator path.
        #[test]
        fn frame_rate_numerator_max_value_round_trips() {
            let fraction: u64 = (u64::from(u32::MAX) << 32) | u64::from(u32::MAX);
            assert_eq!(frame_rate_numerator(fraction), u32::MAX);
        }

        /// All-zero packed value — the documented "no rate" sentinel
        /// the source reader returns when a stream has no rate hint.
        /// The helper must not panic on this; it returns `0`. The
        /// caller is expected to treat `0` as "unknown" rather than
        /// a real frame rate, but that policy is outside the helper.
        #[test]
        fn frame_rate_numerator_zero_packed_returns_zero() {
            assert_eq!(frame_rate_numerator(0), 0);
        }

        /// `MF_MT_FRAME_SIZE` packs `width << 32 | height`. Pin the
        /// canonical 1920x1080 case to lock the high/low halves to
        /// width/height respectively. Mirrors the bug we fixed in
        /// `frame_rate_numerator` where the high/low halves were
        /// transposed at the call site.
        #[test]
        fn parse_frame_size_1080p() {
            let packed: u64 = (1920_u64 << 32) | 0x438_u64; // 1080
            assert_eq!(parse_frame_size(packed), (1920, 1080));
        }

        /// Both halves at `u32::MAX` round-trip — the helper must
        /// not lose the high bit on either cast. Same invariant we
        /// pin for `frame_rate_numerator`.
        #[test]
        fn parse_frame_size_max_u32_round_trips() {
            let packed: u64 = (u64::from(u32::MAX) << 32) | u64::from(u32::MAX);
            assert_eq!(parse_frame_size(packed), (u32::MAX, u32::MAX));
        }

        /// All-zero packed value produces `(0, 0)` rather than
        /// panicking — useful when an upstream consumer treats `0`
        /// as "unknown" instead of `Resolution::new(0, 0)`. The
        /// helper itself does not interpret the sentinel.
        #[test]
        fn parse_frame_size_zero_packed_is_zero_zero() {
            assert_eq!(parse_frame_size(0), (0, 0));
        }

        /// Asymmetric: only the low half is set. Pins that the
        /// width comes from the *high* 32 bits (so it's `0` here)
        /// and the height comes from the *low* 32 bits (so it's
        /// `1080` here) — i.e. the layout is `width:hi | height:lo`,
        /// not the reverse. A regression of this layout would
        /// silently swap width and height for every camera.
        #[test]
        fn parse_frame_size_low_half_only_is_zero_height_pair() {
            assert_eq!(parse_frame_size(1080), (0, 1080));
        }
    }
}

#[cfg(any(not(windows), feature = "docs-only"))]
#[allow(clippy::must_use_candidate)]
pub mod wmf {
    use nokhwa_core::error::NokhwaError;
    use nokhwa_core::types::{
        CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
        KnownCameraControl,
    };
    use std::{borrow::Cow, time::Duration};

    pub fn initialize_mf() -> Result<(), NokhwaError> {
        Err(NokhwaError::NotImplementedError(
            "Not on windows".to_string(),
        ))
    }

    pub fn de_initialize_mf() -> Result<(), NokhwaError> {
        Err(NokhwaError::NotImplementedError(
            "Not on windows".to_string(),
        ))
    }

    pub fn query() -> Result<Vec<CameraInfo>, NokhwaError> {
        Err(NokhwaError::NotImplementedError(
            "Not on windows".to_string(),
        ))
    }

    pub struct MediaFoundationDevice {
        camera: CameraIndex,
    }

    impl MediaFoundationDevice {
        pub fn new(_index: CameraIndex) -> Result<Self, NokhwaError> {
            Ok(MediaFoundationDevice {
                camera: CameraIndex::Index(0),
            })
        }

        pub fn index(&self) -> &CameraIndex {
            &self.camera
        }

        pub fn name(&self) -> String {
            String::new()
        }

        pub fn symlink(&self) -> String {
            String::new()
        }

        pub fn compatible_format_list(&mut self) -> Result<Vec<CameraFormat>, NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn control(&self, _control: KnownCameraControl) -> Result<CameraControl, NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn set_control(
            &mut self,
            _control: KnownCameraControl,
            _value: ControlValueSetter,
        ) -> Result<(), NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn format_refreshed(&mut self) -> Result<CameraFormat, NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn format(&self) -> CameraFormat {
            CameraFormat::default()
        }

        pub fn set_format(&mut self, _format: CameraFormat) -> Result<(), NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn is_stream_open(&self) -> bool {
            false
        }

        pub fn start_stream(&mut self) -> Result<(), NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn raw_bytes(&mut self) -> Result<(Cow<'_, [u8]>, Option<Duration>), NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "Only on Windows".to_string(),
            ))
        }

        pub fn stop_stream(&mut self) {}
    }

    impl Drop for MediaFoundationDevice {
        fn drop(&mut self) {}
    }
}

#[cfg(all(windows, not(feature = "docs-only")))]
mod capture;
#[cfg(all(windows, not(feature = "docs-only")))]
pub use capture::MediaFoundationCaptureDevice;

mod hotplug;
pub use hotplug::MediaFoundationHotplugContext;

/// Non-Windows stub for `MediaFoundationCaptureDevice`.
///
/// Exists so that cross-platform documentation builds (`cargo doc
/// --features docs-only,docs-nolink`) and downstream code that merely
/// references the type can compile on macOS / Linux hosts. Fallible
/// methods return [`NokhwaError::NotImplementedError`]; infallible
/// methods panic via `unreachable!()` — they cannot be reached in
/// practice because `MediaFoundationCaptureDevice::new` errors off
/// Windows, so no value of this stub type can exist at runtime.
///
/// Mirrors the non-Linux stub used by `V4LCaptureDevice`.
#[cfg(any(not(windows), feature = "docs-only"))]
mod stub {
    use nokhwa_core::buffer::Buffer;
    use nokhwa_core::error::NokhwaError;
    use nokhwa_core::traits::{CameraDevice, FrameSource};
    use nokhwa_core::types::{
        ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueSetter,
        FrameFormat, KnownCameraControl, RequestedFormat,
    };
    use std::borrow::Cow;

    /// See module docs for behavior off Windows.
    pub struct MediaFoundationCaptureDevice;

    #[allow(unused_variables)]
    impl MediaFoundationCaptureDevice {
        /// Creates a new capture device using the Media Foundation backend.
        /// # Errors
        /// Always returns [`NokhwaError::NotImplementedError`] off Windows.
        pub fn new(index: &CameraIndex, camera_fmt: RequestedFormat) -> Result<Self, NokhwaError> {
            Err(NokhwaError::NotImplementedError(
                "MediaFoundation only on Windows".to_string(),
            ))
        }
    }

    // Shared error for fallible stub methods.
    fn not_on_this_platform() -> NokhwaError {
        NokhwaError::NotImplementedError("MediaFoundation only on Windows".to_string())
    }

    // Shared panic message for infallible stub methods. These methods
    // cannot return an error and should never be called in practice
    // because `MediaFoundationCaptureDevice::new` errors off Windows,
    // so no `MediaFoundationCaptureDevice` value can exist at runtime.
    #[cold]
    #[inline(never)]
    fn stub_unreachable() -> ! {
        unreachable!("MediaFoundation stub: only available on Windows")
    }

    #[allow(unused_variables)]
    impl CameraDevice for MediaFoundationCaptureDevice {
        fn backend(&self) -> ApiBackend {
            ApiBackend::MediaFoundation
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
    impl FrameSource for MediaFoundationCaptureDevice {
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
        use super::{not_on_this_platform, MediaFoundationCaptureDevice};
        use nokhwa_core::error::NokhwaError;
        use nokhwa_core::format_types::Mjpeg;
        use nokhwa_core::traits::{CameraDevice, FrameSource};
        use nokhwa_core::types::{
            ApiBackend, CameraFormat, CameraIndex, ControlValueSetter, FrameFormat,
            KnownCameraControl, RequestedFormat, RequestedFormatType, Resolution,
        };

        // Pin the contract that the off-Windows stub never hands out a live
        // device: every fallible method returns `NotImplementedError` and
        // `MediaFoundationCaptureDevice::new` errors deterministically.
        // Keeps the docs-only / cross-platform `cargo check` builds honest
        // even though the real backend only links on Windows.

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
        fn new_errors_off_windows() {
            // Can't use `.expect_err` because the stub type intentionally
            // does not implement `Debug` — pattern-match instead.
            match MediaFoundationCaptureDevice::new(
                &CameraIndex::Index(0),
                RequestedFormat::new::<Mjpeg>(RequestedFormatType::AbsoluteHighestFrameRate),
            ) {
                Err(err) => assert_not_implemented(&err),
                Ok(_) => panic!("stub `new` must always error off Windows"),
            }
        }

        #[test]
        fn backend_reports_media_foundation() {
            let dev = MediaFoundationCaptureDevice;
            assert_eq!(dev.backend(), ApiBackend::MediaFoundation);
        }

        #[test]
        fn camera_device_fallible_methods_return_not_implemented() {
            let mut dev = MediaFoundationCaptureDevice;
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
            let mut dev = MediaFoundationCaptureDevice;
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
            let dev = MediaFoundationCaptureDevice;
            assert!(!dev.is_open());
        }

        #[test]
        #[should_panic(expected = "MediaFoundation stub: only available on Windows")]
        fn info_panics_via_stub_unreachable() {
            let dev = MediaFoundationCaptureDevice;
            let _info = dev.info();
        }

        #[test]
        #[should_panic(expected = "MediaFoundation stub: only available on Windows")]
        fn negotiated_format_panics_via_stub_unreachable() {
            let dev = MediaFoundationCaptureDevice;
            let _fmt = dev.negotiated_format();
        }
    }
}

#[cfg(any(not(windows), feature = "docs-only"))]
pub use stub::MediaFoundationCaptureDevice;
