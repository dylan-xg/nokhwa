//! Thin free-function wrappers around typed `objc2-av-foundation` session APIs.
//!
//! These are free functions (not methods on a wrapper struct) because the typed
//! `AVCaptureSession`, `AVCaptureDeviceInput`, and `AVCaptureVideoDataOutput` types
//! from `objc2-av-foundation` are already safe, well-typed, and reference-counted via
//! `Retained<T>`. No additional state needs to be tracked, unlike `AVCaptureDeviceWrapper`
//! in `device.rs` which maintains a `locked` flag.

use crate::callback::AVCaptureVideoCallback;
use crate::device::get_raw_device_info;
use crate::ffi::{
    kCMPixelFormat_24RGB, kCMPixelFormat_422YpCbCr8_yuvs, kCMPixelFormat_8IndexedGray_WhiteIsZero,
    kCMVideoCodecType_JPEG, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use crate::types::AVCaptureDeviceTypeLocal;
use nokhwa_core::{
    error::NokhwaError,
    types::{ApiBackend, CameraIndex, CameraInfo, FrameFormat},
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput,
    AVCaptureDevicePosition, AVCaptureInput, AVCaptureOutput, AVCaptureSession,
    AVCaptureVideoDataOutput, AVMediaTypeVideo,
};
use objc2_foundation::{NSArray, NSString};

/// Returns `Result` for API consistency with callers that propagate `NokhwaError`,
/// even though the current implementation is infallible.
///
/// # Panics
///
/// Panics if the `AVMediaTypeVideo` constant is unavailable on the current
/// platform, which should not happen on any supported Apple platform.
pub fn discovery_session_with_types(
    device_types: &[AVCaptureDeviceTypeLocal],
) -> Result<Retained<AVCaptureDeviceDiscoverySession>, NokhwaError> {
    let refs: Vec<&NSString> = device_types
        .iter()
        .map(|dt| dt.as_av_capture_device_type())
        .collect();
    let device_types_arr = NSArray::from_slice(&refs);

    let media_type_video = unsafe { AVMediaTypeVideo.unwrap() };

    let session = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &device_types_arr,
            Some(media_type_video),
            AVCaptureDevicePosition::Unspecified,
        )
    };

    Ok(session)
}

pub fn discovery_session_devices(session: &AVCaptureDeviceDiscoverySession) -> Vec<CameraInfo> {
    let devices = unsafe { session.devices() };
    let count = devices.count();
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let device = devices.objectAtIndex(index);
        // index is a usize array offset bounded by count (a small device list, well within u32::MAX).
        #[allow(clippy::cast_possible_truncation)]
        result.push(get_raw_device_info(
            CameraIndex::Index(index as u32),
            &device,
        ));
    }
    result
}

pub fn create_device_input(
    capture_device: &AVCaptureDevice,
) -> Result<Retained<AVCaptureDeviceInput>, NokhwaError> {
    unsafe {
        AVCaptureDeviceInput::deviceInputWithDevice_error(capture_device).map_err(|e| {
            let desc = e.localizedDescription();
            NokhwaError::InitializeError {
                backend: nokhwa_core::types::ApiBackend::AVFoundation,
                error: format!("Failed to create input: {desc}"),
            }
        })
    }
}

#[must_use]
pub fn create_video_data_output() -> Retained<AVCaptureVideoDataOutput> {
    unsafe { AVCaptureVideoDataOutput::new() }
}

pub fn output_add_delegate(
    output: &AVCaptureVideoDataOutput,
    delegate: &AVCaptureVideoCallback,
) -> Result<(), NokhwaError> {
    // setSampleBufferDelegate:queue: requires dispatch2 feature which brings in
    // a separate DispatchQueue type. We keep using msg_send! with our own DispatchQueue wrapper.
    unsafe {
        let _: () = objc2::msg_send![
            output,
            setSampleBufferDelegate: delegate.inner(),
            queue: delegate.queue().0
        ];
    };
    Ok(())
}

pub fn output_set_frame_format(
    output: &AVCaptureVideoDataOutput,
    format: FrameFormat,
) -> Result<(), NokhwaError> {
    let cmpixelfmt = match format {
        FrameFormat::YUYV => kCMPixelFormat_422YpCbCr8_yuvs,
        FrameFormat::MJPEG => kCMVideoCodecType_JPEG,
        FrameFormat::GRAY => kCMPixelFormat_8IndexedGray_WhiteIsZero,
        FrameFormat::NV12 => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        FrameFormat::RAWRGB => kCMPixelFormat_24RGB,
        FrameFormat::RAWBGR => {
            return Err(NokhwaError::set_property(
                "setVideoSettings",
                "set frame format",
                "Unsupported frame format BGR",
            ));
        },
    };

    // Build NSDictionary via msg_send! since the typed NSDictionary API
    // requires complex generics. The key is a CFString (toll-free bridged with NSString).
    let ns_number_cls = objc2::class!(NSNumber);
    // cmpixelfmt is a FourCharCode (u32); NSNumber's numberWithInt: takes an i32.
    // The FourCharCode bit pattern is preserved — the value is used as an opaque key, not compared numerically.
    #[allow(clippy::cast_possible_wrap)]
    let obj: *mut AnyObject =
        unsafe { objc2::msg_send![ns_number_cls, numberWithInt: cmpixelfmt as i32] };
    let key = unsafe { crate::ffi::kCVPixelBufferPixelFormatTypeKey } as *mut AnyObject;

    let dict_cls = objc2::class!(NSDictionary);
    let dict: *mut AnyObject =
        unsafe { objc2::msg_send![dict_cls, dictionaryWithObject:obj, forKey:key] };

    // SAFETY: `dict` is a non-null pointer just returned by +[NSDictionary dictionaryWithObject:forKey:].
    // The object is an NSDictionary whose single key is an NSString (kCVPixelBufferPixelFormatTypeKey,
    // a CFString that is toll-free bridged to NSString) and whose value is an NSNumber (AnyObject).
    let dict_ref: &objc2_foundation::NSDictionary<NSString, AnyObject> =
        unsafe { &*(dict as *const _) };
    unsafe { output.setVideoSettings(Some(dict_ref)) };
    Ok(())
}

// -- AVCaptureSession wrapper functions --

#[must_use]
pub fn session_new() -> Retained<AVCaptureSession> {
    unsafe { AVCaptureSession::new() }
}

pub fn session_begin_configuration(session: &AVCaptureSession) {
    unsafe { session.beginConfiguration() }
}

pub fn session_commit_configuration(session: &AVCaptureSession) {
    unsafe { session.commitConfiguration() }
}

pub fn session_add_input(
    session: &AVCaptureSession,
    input: &AVCaptureDeviceInput,
) -> Result<(), NokhwaError> {
    let input_ref: &AVCaptureInput = input;
    if unsafe { session.canAddInput(input_ref) } {
        unsafe { session.addInput(input_ref) };
        return Ok(());
    }
    Err(NokhwaError::set_property(
        "AVCaptureDeviceInput",
        "add new input",
        "Rejected",
    ))
}

pub fn session_remove_input(session: &AVCaptureSession, input: &AVCaptureDeviceInput) {
    let input_ref: &AVCaptureInput = input;
    unsafe { session.removeInput(input_ref) }
}

pub fn session_add_output(
    session: &AVCaptureSession,
    output: &AVCaptureVideoDataOutput,
) -> Result<(), NokhwaError> {
    let output_ref: &AVCaptureOutput = output;
    if unsafe { session.canAddOutput(output_ref) } {
        unsafe { session.addOutput(output_ref) };
        return Ok(());
    }
    Err(NokhwaError::set_property(
        "AVCaptureVideoDataOutput",
        "add new output",
        "Rejected",
    ))
}

pub fn session_remove_output(session: &AVCaptureSession, output: &AVCaptureVideoDataOutput) {
    let output_ref: &AVCaptureOutput = output;
    unsafe { session.removeOutput(output_ref) }
}

pub fn session_is_running(session: &AVCaptureSession) -> bool {
    unsafe { session.isRunning() }
}

pub fn session_start(session: &AVCaptureSession) -> Result<(), NokhwaError> {
    // AssertUnwindSafe: startRunning may trigger ObjC exceptions translated to panics.
    // The &AVCaptureSession reference is not invalidated by a panic — the session object
    // remains in a consistent (non-running) state if startRunning fails.
    let start_stream_fn = std::panic::AssertUnwindSafe(|| {
        unsafe { session.startRunning() };
    });

    if std::panic::catch_unwind(start_stream_fn).is_err() {
        return Err(NokhwaError::OpenStreamError {
            message: "Cannot run AVCaptureSession".to_string(),
            backend: Some(ApiBackend::AVFoundation),
        });
    }
    Ok(())
}

pub fn session_stop(session: &AVCaptureSession) {
    unsafe { session.stopRunning() }
}

pub fn session_is_interrupted(session: &AVCaptureSession) -> bool {
    unsafe { session.isInterrupted() }
}
