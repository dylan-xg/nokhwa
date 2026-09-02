use crate::ffi::{CMFormatDescriptionGetMediaSubType, CMVideoFormatDescriptionGetDimensions};
use crate::ffi::{CMFormatDescriptionRef, CMVideoDimensions};
use crate::session::{discovery_session_devices, discovery_session_with_types};
use crate::types::{AVCaptureDeviceTypeLocal, AVMediaTypeLocal};
use crate::util::raw_fcc_to_frameformat;
use nokhwa_core::{
    error::NokhwaError,
    types::{
        ApiBackend, CameraControl, CameraFormat, CameraIndex, CameraInfo, ControlValueDescription,
        ControlValueSetter, FrameFormat, KnownCameraControl, KnownCameraControlFlag, Resolution,
    },
};
use objc2::rc::Retained;
use objc2::Message;
use objc2_av_foundation::{
    AVCaptureDevice, AVCaptureDeviceFormat, AVCaptureDevicePosition,
    AVCaptureExposureDurationCurrent, AVCaptureExposureMode, AVCaptureExposureTargetBiasCurrent,
    AVCaptureFocusMode, AVCaptureISOCurrent, AVCaptureTorchMode, AVCaptureWhiteBalanceGains,
    AVCaptureWhiteBalanceMode, AVFrameRateRange,
};
use objc2_core_foundation::{CGFloat, CGPoint};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::NSArray;
use std::{cmp::Ordering, collections::BTreeMap, ffi::c_float};

pub fn query() -> Result<Vec<CameraInfo>, NokhwaError> {
    let session = discovery_session_with_types(&[
        AVCaptureDeviceTypeLocal::UltraWide,
        AVCaptureDeviceTypeLocal::WideAngle,
        AVCaptureDeviceTypeLocal::Telephoto,
        AVCaptureDeviceTypeLocal::TrueDepth,
        AVCaptureDeviceTypeLocal::External,
    ])?;
    Ok(discovery_session_devices(&session))
}

// -- Safe wrappers for AVCaptureDevice read-only property accessors --
//
// objc2-av-foundation 0.3.x marks ALL methods as `unsafe` even when they are
// simple read-only property getters with no preconditions beyond having a valid
// receiver.  These thin wrappers eliminate the need for `unsafe` at every call
// site while documenting why the call is sound.

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_localized_name(device: &AVCaptureDevice) -> Retained<objc2_foundation::NSString> {
    unsafe { device.localizedName() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_manufacturer(device: &AVCaptureDevice) -> Retained<objc2_foundation::NSString> {
    unsafe { device.manufacturer() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_position(device: &AVCaptureDevice) -> AVCaptureDevicePosition {
    unsafe { device.position() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_lens_aperture(device: &AVCaptureDevice) -> c_float {
    unsafe { device.lensAperture() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_device_type(
    device: &AVCaptureDevice,
) -> Retained<objc2_av_foundation::AVCaptureDeviceType> {
    unsafe { device.deviceType() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_model_id(device: &AVCaptureDevice) -> Retained<objc2_foundation::NSString> {
    unsafe { device.modelID() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_unique_id(device: &AVCaptureDevice) -> Retained<objc2_foundation::NSString> {
    unsafe { device.uniqueID() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_formats(device: &AVCaptureDevice) -> Retained<NSArray<AVCaptureDeviceFormat>> {
    unsafe { device.formats() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_active_format(device: &AVCaptureDevice) -> Retained<AVCaptureDeviceFormat> {
    unsafe { device.activeFormat() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_in_use_by_another_application(device: &AVCaptureDevice) -> bool {
    unsafe { device.isInUseByAnotherApplication() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_suspended(device: &AVCaptureDevice) -> bool {
    unsafe { device.isSuspended() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_focus_mode(device: &AVCaptureDevice) -> AVCaptureFocusMode {
    unsafe { device.focusMode() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_focus_mode_supported(device: &AVCaptureDevice, mode: AVCaptureFocusMode) -> bool {
    unsafe { device.isFocusModeSupported(mode) }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_focus_poi_supported(device: &AVCaptureDevice) -> bool {
    unsafe { device.isFocusPointOfInterestSupported() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_focus_poi(device: &AVCaptureDevice) -> CGPoint {
    unsafe { device.focusPointOfInterest() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_locking_focus_with_custom_lens_position_supported(device: &AVCaptureDevice) -> bool {
    unsafe { device.isLockingFocusWithCustomLensPositionSupported() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_lens_position(device: &AVCaptureDevice) -> c_float {
    unsafe { device.lensPosition() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_exposure_mode(device: &AVCaptureDevice) -> AVCaptureExposureMode {
    unsafe { device.exposureMode() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_exposure_mode_supported(
    device: &AVCaptureDevice,
    mode: AVCaptureExposureMode,
) -> bool {
    unsafe { device.isExposureModeSupported(mode) }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_exposure_poi_supported(device: &AVCaptureDevice) -> bool {
    unsafe { device.isExposurePointOfInterestSupported() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_exposure_poi(device: &AVCaptureDevice) -> CGPoint {
    unsafe { device.exposurePointOfInterest() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_face_driven_auto_exposure_enabled(device: &AVCaptureDevice) -> bool {
    unsafe { device.isFaceDrivenAutoExposureEnabled() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_automatically_adjusts_face_driven_auto_exposure(device: &AVCaptureDevice) -> bool {
    unsafe { device.automaticallyAdjustsFaceDrivenAutoExposureEnabled() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_exposure_target_bias(device: &AVCaptureDevice) -> c_float {
    unsafe { device.exposureTargetBias() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_min_exposure_target_bias(device: &AVCaptureDevice) -> c_float {
    unsafe { device.minExposureTargetBias() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_max_exposure_target_bias(device: &AVCaptureDevice) -> c_float {
    unsafe { device.maxExposureTargetBias() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_exposure_duration(device: &AVCaptureDevice) -> CMTime {
    unsafe { device.exposureDuration() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_iso(device: &AVCaptureDevice) -> c_float {
    unsafe { device.ISO() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_white_balance_mode(device: &AVCaptureDevice) -> AVCaptureWhiteBalanceMode {
    unsafe { device.whiteBalanceMode() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_white_balance_mode_supported(
    device: &AVCaptureDevice,
    mode: AVCaptureWhiteBalanceMode,
) -> bool {
    unsafe { device.isWhiteBalanceModeSupported(mode) }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_white_balance_gains(device: &AVCaptureDevice) -> AVCaptureWhiteBalanceGains {
    unsafe { device.deviceWhiteBalanceGains() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_gray_world_white_balance_gains(device: &AVCaptureDevice) -> AVCaptureWhiteBalanceGains {
    unsafe { device.grayWorldDeviceWhiteBalanceGains() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_max_white_balance_gain(device: &AVCaptureDevice) -> c_float {
    unsafe { device.maxWhiteBalanceGain() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_locking_white_balance_with_custom_gains_supported(device: &AVCaptureDevice) -> bool {
    unsafe { device.isLockingWhiteBalanceWithCustomDeviceGainsSupported() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_torch_available(device: &AVCaptureDevice) -> bool {
    unsafe { device.isTorchAvailable() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_torch_mode_supported(device: &AVCaptureDevice, mode: AVCaptureTorchMode) -> bool {
    unsafe { device.isTorchModeSupported(mode) }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_torch_mode(device: &AVCaptureDevice) -> AVCaptureTorchMode {
    unsafe { device.torchMode() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_low_light_boost_supported(device: &AVCaptureDevice) -> bool {
    unsafe { device.isLowLightBoostSupported() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_low_light_boost_enabled(device: &AVCaptureDevice) -> bool {
    unsafe { device.isLowLightBoostEnabled() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_video_zoom_factor(device: &AVCaptureDevice) -> CGFloat {
    unsafe { device.videoZoomFactor() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_min_available_video_zoom_factor(device: &AVCaptureDevice) -> CGFloat {
    unsafe { device.minAvailableVideoZoomFactor() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_max_available_video_zoom_factor(device: &AVCaptureDevice) -> CGFloat {
    unsafe { device.maxAvailableVideoZoomFactor() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_geometric_distortion_correction_supported(device: &AVCaptureDevice) -> bool {
    unsafe { device.isGeometricDistortionCorrectionSupported() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_is_geometric_distortion_correction_enabled(device: &AVCaptureDevice) -> bool {
    unsafe { device.isGeometricDistortionCorrectionEnabled() }
}

// SAFETY: `deviceWithUniqueID:` is a class method that returns nil for unknown IDs;
// caller handles the `None` case.
fn device_with_unique_id(id: &objc2_foundation::NSString) -> Option<Retained<AVCaptureDevice>> {
    unsafe { AVCaptureDevice::deviceWithUniqueID(id) }
}

// -- Safe wrappers for AVCaptureDevice mutating operations --
// These require the device to be locked for configuration first.

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_active_format(device: &AVCaptureDevice, format: &AVCaptureDeviceFormat) {
    unsafe { device.setActiveFormat(format) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_active_video_min_frame_duration(device: &AVCaptureDevice, duration: CMTime) {
    unsafe { device.setActiveVideoMinFrameDuration(duration) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_active_video_max_frame_duration(device: &AVCaptureDevice, duration: CMTime) {
    unsafe { device.setActiveVideoMaxFrameDuration(duration) }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDevice` reference.
fn device_active_video_min_frame_duration(device: &AVCaptureDevice) -> CMTime {
    unsafe { device.activeVideoMinFrameDuration() }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_focus_mode(device: &AVCaptureDevice, mode: AVCaptureFocusMode) {
    unsafe { device.setFocusMode(mode) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_focus_poi(device: &AVCaptureDevice, point: CGPoint) {
    unsafe { device.setFocusPointOfInterest(point) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_focus_mode_locked_with_lens_position(device: &AVCaptureDevice, position: c_float) {
    unsafe { device.setFocusModeLockedWithLensPosition_completionHandler(position, None) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_exposure_mode(device: &AVCaptureDevice, mode: AVCaptureExposureMode) {
    unsafe { device.setExposureMode(mode) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_exposure_poi(device: &AVCaptureDevice, point: CGPoint) {
    unsafe { device.setExposurePointOfInterest(point) }
}

// SAFETY: Mutating method on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_exposure_custom(device: &AVCaptureDevice, duration: CMTime, iso: c_float) {
    unsafe { device.setExposureModeCustomWithDuration_ISO_completionHandler(duration, iso, None) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_exposure_target_bias(device: &AVCaptureDevice, bias: c_float) {
    unsafe { device.setExposureTargetBias_completionHandler(bias, None) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_auto_adjusts_face_driven_auto_exposure(device: &AVCaptureDevice, enabled: bool) {
    unsafe { device.setAutomaticallyAdjustsFaceDrivenAutoExposureEnabled(enabled) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_white_balance_mode(device: &AVCaptureDevice, mode: AVCaptureWhiteBalanceMode) {
    unsafe { device.setWhiteBalanceMode(mode) }
}

// SAFETY: Mutating method on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_white_balance_gains(device: &AVCaptureDevice, gains: AVCaptureWhiteBalanceGains) {
    unsafe {
        device.setWhiteBalanceModeLockedWithDeviceWhiteBalanceGains_completionHandler(gains, None);
    }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_auto_low_light_boost(device: &AVCaptureDevice, enabled: bool) {
    unsafe { device.setAutomaticallyEnablesLowLightBoostWhenAvailable(enabled) }
}

// SAFETY: Mutating method on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_ramp_to_video_zoom_factor(device: &AVCaptureDevice, factor: CGFloat, rate: c_float) {
    unsafe { device.rampToVideoZoomFactor_withRate(factor, rate) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_torch_mode(device: &AVCaptureDevice, mode: AVCaptureTorchMode) {
    unsafe { device.setTorchMode(mode) }
}

// SAFETY: Property setter on a valid `AVCaptureDevice` reference.
// Caller must lock the device for configuration before calling this.
fn device_set_geometric_distortion_correction_enabled(device: &AVCaptureDevice, enabled: bool) {
    unsafe { device.setGeometricDistortionCorrectionEnabled(enabled) }
}

// SAFETY: `lockForConfiguration` is safe to call on a valid `AVCaptureDevice`;
// it returns an error rather than causing UB if the device is unavailable.
fn device_lock_for_configuration(
    device: &AVCaptureDevice,
) -> Result<(), Retained<objc2_foundation::NSError>> {
    unsafe { device.lockForConfiguration() }
}

// SAFETY: `unlockForConfiguration` is safe to call after a successful lock.
fn device_unlock_for_configuration(device: &AVCaptureDevice) {
    unsafe { device.unlockForConfiguration() }
}

// -- Safe wrappers for AVCaptureDeviceFormat read-only property accessors --

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_media_type(format: &AVCaptureDeviceFormat) -> Retained<objc2_foundation::NSString> {
    unsafe { format.mediaType() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_frame_rate_ranges(format: &AVCaptureDeviceFormat) -> Retained<NSArray<AVFrameRateRange>> {
    unsafe { format.videoSupportedFrameRateRanges() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_description(
    format: &AVCaptureDeviceFormat,
) -> Retained<objc2_core_media::CMFormatDescription> {
    unsafe { format.formatDescription() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_min_exposure_duration(format: &AVCaptureDeviceFormat) -> CMTime {
    unsafe { format.minExposureDuration() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_max_exposure_duration(format: &AVCaptureDeviceFormat) -> CMTime {
    unsafe { format.maxExposureDuration() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_min_iso(format: &AVCaptureDeviceFormat) -> c_float {
    unsafe { format.minISO() }
}

// SAFETY: Read-only property accessor on a valid `AVCaptureDeviceFormat` reference.
fn format_max_iso(format: &AVCaptureDeviceFormat) -> c_float {
    unsafe { format.maxISO() }
}

// -- Safe wrappers for AVFrameRateRange read-only property accessors --

// SAFETY: Read-only property accessor on a valid `AVFrameRateRange` reference.
fn range_max_frame_rate(range: &AVFrameRateRange) -> f64 {
    unsafe { range.maxFrameRate() }
}

// SAFETY: Read-only property accessor on a valid `AVFrameRateRange` reference.
fn range_min_frame_duration(range: &AVFrameRateRange) -> CMTime {
    unsafe { range.minFrameDuration() }
}

// -- Safe wrappers for CMFormatDescription C FFI calls --

// SAFETY: The reference is converted to a raw pointer for the C FFI call. The pointer
// remains valid for the duration of the call because the borrow on `desc` keeps the
// underlying `Retained<CMFormatDescription>` alive.
fn cm_video_format_get_dimensions(
    desc: &objc2_core_media::CMFormatDescription,
) -> CMVideoDimensions {
    let ptr = std::ptr::from_ref(desc) as CMFormatDescriptionRef;
    unsafe { CMVideoFormatDescriptionGetDimensions(ptr) }
}

// SAFETY: The reference is converted to a raw pointer for the C FFI call. The pointer
// remains valid for the duration of the call because the borrow on `desc` keeps the
// underlying `Retained<CMFormatDescription>` alive.
fn cm_format_get_media_sub_type(desc: &objc2_core_media::CMFormatDescription) -> u32 {
    let ptr = std::ptr::from_ref(desc) as CMFormatDescriptionRef;
    unsafe { CMFormatDescriptionGetMediaSubType(ptr) }
}

// -- Safe wrappers for global sentinel constants --
// These are extern statics in objc2-av-foundation that require `unsafe` to access.

// SAFETY: Read-only global sentinel constant; always valid on supported Apple platforms.
fn exposure_target_bias_current() -> c_float {
    unsafe { AVCaptureExposureTargetBiasCurrent }
}

// SAFETY: Read-only global sentinel constant; always valid on supported Apple platforms.
fn exposure_duration_current() -> CMTime {
    unsafe { AVCaptureExposureDurationCurrent }
}

// SAFETY: Read-only global sentinel constant; always valid on supported Apple platforms.
fn iso_current() -> c_float {
    unsafe { AVCaptureISOCurrent }
}

pub(crate) fn get_raw_device_info(index: CameraIndex, device: &AVCaptureDevice) -> CameraInfo {
    let name = device_localized_name(device);
    let manufacturer = device_manufacturer(device);
    let position = device_position(device);
    let lens_aperture = device_lens_aperture(device);
    let device_type = device_device_type(device);
    let model_id = device_model_id(device);
    let description =
        format!("{manufacturer}: {model_id} - {device_type}, {position:?} f{lens_aperture}");
    let misc = device_unique_id(device);

    CameraInfo::new(
        name.to_string().as_ref(),
        &description,
        misc.to_string().as_ref(),
        false,
        index,
    )
}

#[derive(Debug)]
pub(crate) struct AVCaptureDeviceFormatWrapper {
    /// Retained to prevent deallocation while the wrapper is alive.
    #[allow(dead_code)]
    pub(crate) internal: Retained<AVCaptureDeviceFormat>,
    pub(crate) resolution: CMVideoDimensions,
    pub(crate) fps_list: Vec<f64>,
    pub(crate) fourcc: FrameFormat,
}

impl AVCaptureDeviceFormatWrapper {
    pub(crate) fn try_from_format(format: &AVCaptureDeviceFormat) -> Result<Self, NokhwaError> {
        let media_type = format_media_type(format);
        let media_type_local = AVMediaTypeLocal::try_from(media_type.as_ref())?;
        if media_type_local != AVMediaTypeLocal::Video {
            return Err(NokhwaError::structure("AVMediaType", "Not Video"));
        }

        let frame_rate_ranges = format_frame_rate_ranges(format);
        let mut fps_list: Vec<f64> = Vec::new();
        for i in 0..frame_rate_ranges.count() {
            let range = frame_rate_ranges.objectAtIndex(i);
            let max_fps = range_max_frame_rate(&range);
            // Only emit the maxFrameRate endpoint of each AVFrameRateRange.
            // set_all() matches solely against maxFrameRate (using the
            // range's minFrameDuration to clamp delivery rate); emitting
            // minFrameRate here would produce CameraFormat values that
            // compatible_formats() reports as valid but set_format() cannot
            // find, violating the contract that every enumerated entry is
            // settable.
            fps_list.push(max_fps);
        }
        fps_list.sort_by(|n, m| n.partial_cmp(m).unwrap_or(Ordering::Equal));
        fps_list.dedup();

        let description_obj = format_description(format);
        let resolution = cm_video_format_get_dimensions(&description_obj);
        let fcc_raw = cm_format_get_media_sub_type(&description_obj);

        #[allow(non_upper_case_globals)]
        let Some(fourcc) = raw_fcc_to_frameformat(fcc_raw) else {
            return Err(NokhwaError::structure(
                "FourCharCode",
                format!("Unknown FourCharCode {fcc_raw:?}"),
            ));
        };

        Ok(AVCaptureDeviceFormatWrapper {
            internal: format.retain(),
            resolution,
            fps_list,
            fourcc,
        })
    }
}

pub struct AVCaptureDeviceWrapper {
    inner: Retained<AVCaptureDevice>,
    device: CameraInfo,
    locked: bool,
}

impl AVCaptureDeviceWrapper {
    #[must_use]
    pub fn inner(&self) -> &AVCaptureDevice {
        &self.inner
    }
}

impl AVCaptureDeviceWrapper {
    pub fn new(index: &CameraIndex) -> Result<Self, NokhwaError> {
        match &index {
            CameraIndex::Index(idx) => {
                let devices = query()?;

                match devices.get(*idx as usize) {
                    Some(device) => Ok(Self::from_id(&device.misc(), Some(index.clone()))?),
                    None => Err(NokhwaError::open_device(
                        idx.to_string(),
                        "device not found",
                    )),
                }
            },
            CameraIndex::String(id) => {
                // A pure-numeric string is a positional index, not an AVF
                // unique ID — `open(CameraIndex::String("0"))` must reach the
                // same device as `open(CameraIndex::Index(0))`. The session
                // layer routes URL-like strings (rtsp://, http://, file://)
                // to GStreamer before they arrive here, so anything that
                // reaches this arm is either a number or an AVF unique-ID
                // string.
                if let Ok(index) = id.parse::<u32>() {
                    return Self::new(&CameraIndex::Index(index));
                }
                Ok(Self::from_id(id, None)?)
            },
        }
    }

    pub fn from_id(id: &str, index_hint: Option<CameraIndex>) -> Result<Self, NokhwaError> {
        let nsstr_id = objc2_foundation::NSString::from_str(id);
        let capture = device_with_unique_id(&nsstr_id);
        let capture =
            capture.ok_or_else(|| NokhwaError::open_device(id.to_string(), "Device is null"))?;

        let camera_info = get_raw_device_info(
            index_hint.unwrap_or_else(|| CameraIndex::String(id.to_string())),
            &capture,
        );

        Ok(AVCaptureDeviceWrapper {
            inner: capture,
            device: camera_info,
            locked: false,
        })
    }

    #[must_use]
    pub fn info(&self) -> &CameraInfo {
        &self.device
    }

    pub(crate) fn supported_formats_raw(&self) -> Vec<AVCaptureDeviceFormatWrapper> {
        let formats = device_formats(&self.inner);
        let mut result = Vec::new();
        for i in 0..formats.count() {
            let format = formats.objectAtIndex(i);
            // skip unsupported formats silently
            if let Ok(f) = AVCaptureDeviceFormatWrapper::try_from_format(&format) {
                result.push(f);
            }
        }
        result
    }

    pub fn supported_formats(&self) -> Result<Vec<CameraFormat>, NokhwaError> {
        Ok(self
            .supported_formats_raw()
            .iter()
            .flat_map(|av_fmt| {
                let resolution = av_fmt.resolution;
                av_fmt.fps_list.iter().map(move |fps_f64| {
                    // fps_f64 comes from AVFrameRateRange (always positive, max ~240 for Apple devices).
                    // width/height come from CMVideoDimensions (always non-negative).
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let fps = *fps_f64 as u32;
                    #[allow(clippy::cast_sign_loss)]
                    let resolution =
                        Resolution::new(resolution.width as u32, resolution.height as u32);
                    CameraFormat::new(resolution, av_fmt.fourcc, fps)
                })
            })
            .filter(|x| x.frame_rate() != 0)
            .collect())
    }

    #[must_use]
    pub fn already_in_use(&self) -> bool {
        device_is_in_use_by_another_application(&self.inner)
    }

    #[must_use]
    pub fn is_suspended(&self) -> bool {
        device_is_suspended(&self.inner)
    }

    pub fn lock(&mut self) -> Result<(), NokhwaError> {
        if self.locked {
            return Ok(());
        }
        if self.already_in_use() {
            return Err(NokhwaError::InitializeError {
                backend: ApiBackend::AVFoundation,
                error: "Already in use".to_string(),
            });
        }
        device_lock_for_configuration(&self.inner).map_err(|e| {
            let desc = e.localizedDescription();
            NokhwaError::set_property("lockForConfiguration", "Locked", desc.to_string())
        })?;
        self.locked = true;
        Ok(())
    }

    pub fn unlock(&mut self) {
        if self.locked {
            self.locked = false;
            device_unlock_for_configuration(&self.inner);
        }
    }

    pub fn set_all(&mut self, descriptor: CameraFormat) -> Result<(), NokhwaError> {
        self.lock()?;
        let formats = device_formats(&self.inner);

        let mut selected_format: Option<Retained<AVCaptureDeviceFormat>> = None;
        let mut selected_min_frame_duration: Option<CMTime> = None;

        for i in 0..formats.count() {
            let format = formats.objectAtIndex(i);
            let fmt_desc = format_description(&format);
            let dimensions = cm_video_format_get_dimensions(&fmt_desc);

            // Resolution width/height are u32 from nokhwa; CMVideoDimensions uses i32.
            // Values from real cameras are always non-negative and fit in i32.
            #[allow(clippy::cast_possible_wrap)]
            if dimensions.height == descriptor.resolution().height() as i32
                && dimensions.width == descriptor.resolution().width() as i32
            {
                let ranges = format_frame_rate_ranges(&format);
                for j in 0..ranges.count() {
                    let range = ranges.objectAtIndex(j);
                    let max_fps = range_max_frame_rate(&range);
                    if (f64::from(descriptor.frame_rate()) - max_fps).abs() < 0.999 {
                        selected_format = Some(format.clone());
                        selected_min_frame_duration = Some(range_min_frame_duration(&range));
                        break;
                    }
                }
                if selected_format.is_some() {
                    break;
                }
            }
        }

        let (Some(format), Some(min_duration)) = (selected_format, selected_min_frame_duration)
        else {
            // Release the configuration lock acquired above; without this the
            // device stays locked until Drop and subsequent lock() calls are
            // silently skipped.
            self.unlock();
            return Err(NokhwaError::set_property(
                "CameraFormat",
                descriptor.to_string(),
                "Not Found/Rejected/Unsupported",
            ));
        };

        device_set_active_format(&self.inner, &format);
        device_set_active_video_min_frame_duration(&self.inner, min_duration);
        device_set_active_video_max_frame_duration(&self.inner, min_duration);
        self.unlock();
        Ok(())
    }

    // get_controls builds the full AVFoundation control set in one pass.
    // Splitting it into smaller helpers would scatter semantically cohesive
    // groups (focus / exposure / wb / torch / zoom) across many private fns
    // without improving readability.
    #[allow(clippy::too_many_lines)]
    pub fn get_controls(&self) -> Result<Vec<CameraControl>, NokhwaError> {
        let active_format = device_active_format(&self.inner);
        let mut controls = vec![];

        // Focus modes
        let focus_current = device_focus_mode(&self.inner);
        let focus_locked = device_is_focus_mode_supported(&self.inner, AVCaptureFocusMode::Locked);
        let focus_auto = device_is_focus_mode_supported(&self.inner, AVCaptureFocusMode::AutoFocus);
        let focus_continuous =
            device_is_focus_mode_supported(&self.inner, AVCaptureFocusMode::ContinuousAutoFocus);

        {
            let mut supported_focus_values = vec![];
            if focus_locked {
                supported_focus_values.push(0);
            }
            if focus_auto {
                supported_focus_values.push(1);
            }
            if focus_continuous {
                supported_focus_values.push(2);
            }

            // focus_current.0 is NSInteger (isize); the enum variants fit in i64.
            #[allow(clippy::cast_possible_truncation)]
            controls.push(CameraControl::new(
                KnownCameraControl::Focus,
                "FocusMode".to_string(),
                ControlValueDescription::Enum {
                    value: focus_current.0 as i64,
                    possible: supported_focus_values,
                    default: focus_current.0 as i64,
                },
                vec![],
                true,
            ));
        }

        let focus_poi_supported = device_is_focus_poi_supported(&self.inner);
        let focus_poi = device_focus_poi(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Other(0),
            "FocusPointOfInterest".to_string(),
            ControlValueDescription::Point {
                value: (focus_poi.x, focus_poi.y),
                default: (0.5, 0.5),
            },
            disabled_if_unsupported(focus_poi_supported),
            focus_auto || focus_continuous,
        ));

        let focus_manual = device_is_locking_focus_with_custom_lens_position_supported(&self.inner);
        let focus_lenspos: c_float = device_lens_position(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Other(1),
            "FocusManualLensPosition".to_string(),
            ControlValueDescription::FloatRange {
                min: 0.0,
                max: 1.0,
                value: f64::from(focus_lenspos),
                step: f64::MIN_POSITIVE,
                default: 1.0,
            },
            disabled_if_unsupported(focus_manual),
            focus_manual,
        ));

        // Exposure modes
        let exposure_current = device_exposure_mode(&self.inner);
        let exposure_locked =
            device_is_exposure_mode_supported(&self.inner, AVCaptureExposureMode::Locked);
        let exposure_auto =
            device_is_exposure_mode_supported(&self.inner, AVCaptureExposureMode::AutoExpose);
        let exposure_continuous = device_is_exposure_mode_supported(
            &self.inner,
            AVCaptureExposureMode::ContinuousAutoExposure,
        );
        let exposure_custom =
            device_is_exposure_mode_supported(&self.inner, AVCaptureExposureMode::Custom);
        // True only when the device is *currently* in Custom mode (raw value 3).
        // ExposureDuration and ISO are only manually settable in Custom mode;
        // gate their writability on the current mode, not mere support.
        let exposure_is_custom = exposure_current.0 == AVCaptureExposureMode::Custom.0;

        {
            let mut supported_exposure_values = vec![];
            if exposure_locked {
                supported_exposure_values.push(0);
            }
            if exposure_auto {
                supported_exposure_values.push(1);
            }
            if exposure_continuous {
                supported_exposure_values.push(2);
            }
            if exposure_custom {
                supported_exposure_values.push(3);
            }

            // exposure_current.0 is NSInteger (isize); enum variants fit in i64.
            #[allow(clippy::cast_possible_truncation)]
            controls.push(CameraControl::new(
                KnownCameraControl::Exposure,
                "ExposureMode".to_string(),
                ControlValueDescription::Enum {
                    value: exposure_current.0 as i64,
                    possible: supported_exposure_values,
                    default: exposure_current.0 as i64,
                },
                vec![],
                true,
            ));
        }

        let exposure_poi_supported = device_is_exposure_poi_supported(&self.inner);
        let exposure_poi = device_exposure_poi(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Other(2),
            "ExposurePointOfInterest".to_string(),
            ControlValueDescription::Point {
                value: (exposure_poi.x, exposure_poi.y),
                default: (0.5, 0.5),
            },
            disabled_if_unsupported(exposure_poi_supported),
            exposure_auto || exposure_continuous,
        ));

        let exposure_face_driven_supported =
            device_is_face_driven_auto_exposure_enabled(&self.inner);
        let exposure_face_driven =
            device_automatically_adjusts_face_driven_auto_exposure(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Other(3),
            "ExposureFaceDriven".to_string(),
            ControlValueDescription::Boolean {
                value: exposure_face_driven,
                default: false,
            },
            disabled_if_unsupported(exposure_face_driven_supported),
            exposure_poi_supported,
        ));

        let exposure_bias: c_float = device_exposure_target_bias(&self.inner);
        let exposure_bias_min: c_float = device_min_exposure_target_bias(&self.inner);
        let exposure_bias_max: c_float = device_max_exposure_target_bias(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Other(4),
            "ExposureBiasTarget".to_string(),
            ControlValueDescription::FloatRange {
                min: f64::from(exposure_bias_min),
                max: f64::from(exposure_bias_max),
                value: f64::from(exposure_bias),
                step: f64::from(f32::MIN_POSITIVE),
                default: f64::from(exposure_target_bias_current()),
            },
            vec![],
            true,
        ));

        let exposure_duration = device_exposure_duration(&self.inner);
        let exposure_duration_min = format_min_exposure_duration(&active_format);
        let exposure_duration_max = format_max_exposure_duration(&active_format);

        controls.push(CameraControl::new(
            KnownCameraControl::Gamma,
            "ExposureDuration".to_string(),
            ControlValueDescription::IntegerRange {
                min: exposure_duration_min.value,
                max: exposure_duration_max.value,
                value: exposure_duration.value,
                step: 1,
                default: exposure_duration_current().value,
            },
            if exposure_is_custom {
                vec![KnownCameraControlFlag::Volatile]
            } else {
                vec![
                    KnownCameraControlFlag::ReadOnly,
                    KnownCameraControlFlag::Volatile,
                ]
            },
            exposure_is_custom,
        ));

        let exposure_iso: c_float = device_iso(&self.inner);
        let exposure_iso_min: c_float = format_min_iso(&active_format);
        let exposure_iso_max: c_float = format_max_iso(&active_format);

        controls.push(CameraControl::new(
            KnownCameraControl::Brightness,
            "ExposureISO".to_string(),
            ControlValueDescription::FloatRange {
                min: f64::from(exposure_iso_min),
                max: f64::from(exposure_iso_max),
                value: f64::from(exposure_iso),
                step: f64::from(f32::MIN_POSITIVE),
                default: f64::from(iso_current()),
            },
            if exposure_is_custom {
                vec![KnownCameraControlFlag::Volatile]
            } else {
                vec![
                    KnownCameraControlFlag::ReadOnly,
                    KnownCameraControlFlag::Volatile,
                ]
            },
            exposure_is_custom,
        ));

        let lens_aperture: c_float = device_lens_aperture(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Iris,
            "LensAperture".to_string(),
            ControlValueDescription::Float {
                value: f64::from(lens_aperture),
                default: f64::from(lens_aperture),
                step: f64::from(lens_aperture),
            },
            vec![KnownCameraControlFlag::ReadOnly],
            false,
        ));

        // White balance
        let white_balance_current = device_white_balance_mode(&self.inner);
        let white_balance_manual =
            device_is_white_balance_mode_supported(&self.inner, AVCaptureWhiteBalanceMode::Locked);
        let white_balance_auto = device_is_white_balance_mode_supported(
            &self.inner,
            AVCaptureWhiteBalanceMode::AutoWhiteBalance,
        );
        let white_balance_continuous = device_is_white_balance_mode_supported(
            &self.inner,
            AVCaptureWhiteBalanceMode::ContinuousAutoWhiteBalance,
        );

        {
            let mut possible = vec![];
            if white_balance_manual {
                possible.push(0);
            }
            if white_balance_auto {
                possible.push(1);
            }
            if white_balance_continuous {
                possible.push(2);
            }

            // white_balance_current.0 is NSInteger (isize); enum variants fit in i64.
            #[allow(clippy::cast_possible_truncation)]
            controls.push(CameraControl::new(
                KnownCameraControl::WhiteBalance,
                "WhiteBalanceMode".to_string(),
                ControlValueDescription::Enum {
                    value: white_balance_current.0 as i64,
                    possible,
                    default: 0,
                },
                vec![],
                true,
            ));
        }

        let white_balance_gains = device_white_balance_gains(&self.inner);
        let white_balance_default = device_gray_world_white_balance_gains(&self.inner);
        let white_balance_max_scalar: c_float = device_max_white_balance_gain(&self.inner);
        let white_balance_max = AVCaptureWhiteBalanceGains {
            redGain: white_balance_max_scalar,
            greenGain: white_balance_max_scalar,
            blueGain: white_balance_max_scalar,
        };
        let white_balance_gain_supported =
            device_is_locking_white_balance_with_custom_gains_supported(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Gain,
            "WhiteBalanceGain".to_string(),
            ControlValueDescription::RGB {
                value: (
                    f64::from(white_balance_gains.redGain),
                    f64::from(white_balance_gains.greenGain),
                    f64::from(white_balance_gains.blueGain),
                ),
                max: (
                    f64::from(white_balance_max.redGain),
                    f64::from(white_balance_max.greenGain),
                    f64::from(white_balance_max.blueGain),
                ),
                default: (
                    f64::from(white_balance_default.redGain),
                    f64::from(white_balance_default.greenGain),
                    f64::from(white_balance_default.blueGain),
                ),
            },
            disabled_if_unsupported(white_balance_gain_supported),
            white_balance_gain_supported,
        ));

        // Torch
        let has_torch = device_is_torch_available(&self.inner);
        let torch_off = device_is_torch_mode_supported(&self.inner, AVCaptureTorchMode::Off);
        let torch_on = device_is_torch_mode_supported(&self.inner, AVCaptureTorchMode::On);
        let torch_auto = device_is_torch_mode_supported(&self.inner, AVCaptureTorchMode::Auto);

        {
            let mut possible = vec![];
            if torch_off {
                possible.push(0);
            }
            if torch_on {
                possible.push(1);
            }
            if torch_auto {
                possible.push(2);
            }

            let torch_mode_current = device_torch_mode(&self.inner);

            // torch_mode_current.0 is NSInteger (isize); enum variants fit in i64.
            #[allow(clippy::cast_possible_truncation)]
            controls.push(CameraControl::new(
                KnownCameraControl::Other(5),
                "TorchMode".to_string(),
                ControlValueDescription::Enum {
                    value: torch_mode_current.0 as i64,
                    possible,
                    default: 0,
                },
                disabled_if_unsupported(has_torch),
                has_torch,
            ));
        }

        // Low light boost
        let has_llb = device_is_low_light_boost_supported(&self.inner);
        let llb_enabled = device_is_low_light_boost_enabled(&self.inner);

        {
            controls.push(CameraControl::new(
                KnownCameraControl::BacklightComp,
                "LowLightCompensation".to_string(),
                ControlValueDescription::Boolean {
                    value: llb_enabled,
                    default: false,
                },
                disabled_if_unsupported(has_llb),
                has_llb,
            ));
        }

        // Zoom
        let zoom_current: CGFloat = device_video_zoom_factor(&self.inner);
        let zoom_min: CGFloat = device_min_available_video_zoom_factor(&self.inner);
        let zoom_max: CGFloat = device_max_available_video_zoom_factor(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Zoom,
            "Zoom".to_string(),
            ControlValueDescription::FloatRange {
                min: zoom_min,
                max: zoom_max,
                value: zoom_current,
                step: f64::from(f32::MIN_POSITIVE),
                default: 1.0,
            },
            vec![],
            true,
        ));

        // Geometric distortion correction
        let distortion_correction_supported =
            device_is_geometric_distortion_correction_supported(&self.inner);
        let distortion_correction_current_value =
            device_is_geometric_distortion_correction_enabled(&self.inner);

        controls.push(CameraControl::new(
            KnownCameraControl::Other(6),
            "DistortionCorrection".to_string(),
            ControlValueDescription::Boolean {
                value: distortion_correction_current_value,
                default: false,
            },
            disabled_if_unsupported(distortion_correction_supported),
            distortion_correction_supported,
        ));

        Ok(controls)
    }

    // set_control dispatches to all AVFoundation control arms in one function.
    // Splitting would fragment semantically cohesive control-arm logic across
    // many private helpers without improving readability.
    #[allow(clippy::too_many_lines)]
    pub fn set_control(
        &mut self,
        id: KnownCameraControl,
        value: &ControlValueSetter,
    ) -> Result<(), NokhwaError> {
        let rc = self.get_controls()?;
        let controls = rc
            .iter()
            .map(|cc| (cc.control(), cc))
            .collect::<BTreeMap<_, _>>();

        // setter as isize: i64 enum values from nokhwa controls fit in isize on all Apple targets.
        // f64 as c_float: user-supplied float values are expected to be within the valid range.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        match id {
            KnownCameraControl::Brightness => {
                let isoctrl = get_and_check_control(&controls, &id, value)?;

                let current_duration = exposure_duration_current();
                let new_iso = extract_float(value, &id)? as c_float;

                verify_or_error(isoctrl, value, &id)?;

                device_set_exposure_custom(&self.inner, current_duration, new_iso);

                Ok(())
            },
            KnownCameraControl::Gamma => {
                let duration_ctrl = get_and_check_control(&controls, &id, value)?;

                let current_duration = device_exposure_duration(&self.inner);
                let current_iso = iso_current();
                let new_duration = CMTime {
                    value: extract_integer(value, &id)?,
                    timescale: current_duration.timescale,
                    flags: current_duration.flags,
                    epoch: current_duration.epoch,
                };

                verify_or_error(duration_ctrl, value, &id)?;

                device_set_exposure_custom(&self.inner, new_duration, current_iso);

                Ok(())
            },
            KnownCameraControl::WhiteBalance => {
                let wb_enum_value = get_and_check_control(&controls, &id, value)?;

                let setter = extract_enum(value, &id)?;

                verify_or_error(wb_enum_value, value, &id)?;

                device_set_white_balance_mode(
                    &self.inner,
                    AVCaptureWhiteBalanceMode(setter as isize),
                );

                Ok(())
            },
            KnownCameraControl::BacklightComp => {
                let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                let setter = extract_boolean(value, &id)?;

                verify_or_error(ctrlvalue, value, &id)?;

                device_set_auto_low_light_boost(&self.inner, setter);

                Ok(())
            },
            KnownCameraControl::Gain => {
                let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                let (r, g, b) = value.as_rgb().ok_or_else(|| {
                    NokhwaError::set_property(id.to_string(), value.to_string(), "Expected RGB")
                })?;

                verify_or_error(ctrlvalue, value, &id)?;

                let gains = AVCaptureWhiteBalanceGains {
                    redGain: *r as c_float,
                    greenGain: *g as c_float,
                    blueGain: *b as c_float,
                };
                device_set_white_balance_gains(&self.inner, gains);

                Ok(())
            },
            KnownCameraControl::Zoom => {
                let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                let setter = extract_float(value, &id)? as CGFloat;

                verify_or_error(ctrlvalue, value, &id)?;

                device_ramp_to_video_zoom_factor(&self.inner, setter, 1.0_f32);

                Ok(())
            },
            KnownCameraControl::Exposure => {
                let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                let setter = extract_enum(value, &id)?;

                verify_or_error(ctrlvalue, value, &id)?;

                device_set_exposure_mode(&self.inner, AVCaptureExposureMode(setter as isize));

                Ok(())
            },
            KnownCameraControl::Iris => Err(NokhwaError::set_property(
                id.to_string(),
                value.to_string(),
                "Read Only",
            )),
            KnownCameraControl::Focus => {
                let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                let setter = extract_enum(value, &id)?;

                verify_or_error(ctrlvalue, value, &id)?;

                device_set_focus_mode(&self.inner, AVCaptureFocusMode(setter as isize));

                Ok(())
            },
            KnownCameraControl::Other(i) => match i {
                0 => {
                    // Focus point of interest
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = value
                        .as_point()
                        .ok_or_else(|| {
                            NokhwaError::set_property(
                                id.to_string(),
                                value.to_string(),
                                "Expected Point",
                            )
                        })
                        .map(|(x, y)| CGPoint {
                            x: *x as CGFloat,
                            y: *y as CGFloat,
                        })?;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_focus_poi(&self.inner, setter);

                    Ok(())
                },
                1 => {
                    // Focus manual lens position
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = extract_float(value, &id)? as c_float;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_focus_mode_locked_with_lens_position(&self.inner, setter);

                    Ok(())
                },
                2 => {
                    // Exposure point of interest
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = value
                        .as_point()
                        .ok_or_else(|| {
                            NokhwaError::set_property(
                                id.to_string(),
                                value.to_string(),
                                "Expected Point",
                            )
                        })
                        .map(|(x, y)| CGPoint {
                            x: *x as CGFloat,
                            y: *y as CGFloat,
                        })?;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_exposure_poi(&self.inner, setter);

                    Ok(())
                },
                3 => {
                    // Face-driven auto exposure
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = extract_boolean(value, &id)?;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_auto_adjusts_face_driven_auto_exposure(&self.inner, setter);

                    Ok(())
                },
                4 => {
                    // Exposure target bias
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = extract_float(value, &id)? as c_float;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_exposure_target_bias(&self.inner, setter);

                    Ok(())
                },
                5 => {
                    // Torch mode
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = extract_enum(value, &id)?;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_torch_mode(&self.inner, AVCaptureTorchMode(setter as isize));

                    Ok(())
                },
                6 => {
                    // Geometric distortion correction
                    let ctrlvalue = get_and_check_control(&controls, &id, value)?;

                    let setter = extract_boolean(value, &id)?;

                    verify_or_error(ctrlvalue, value, &id)?;

                    device_set_geometric_distortion_correction_enabled(&self.inner, setter);

                    Ok(())
                },
                _ => Err(NokhwaError::set_property(
                    id.to_string(),
                    value.to_string(),
                    "Unknown Control",
                )),
            },
            _ => Err(NokhwaError::set_property(
                id.to_string(),
                value.to_string(),
                "Unknown Control",
            )),
        }
    }

    pub fn active_format(&self) -> Result<CameraFormat, NokhwaError> {
        let af = device_active_format(&self.inner);
        let avf_format = AVCaptureDeviceFormatWrapper::try_from_format(&af)?;
        // fps and resolution casts: same safe FFI boundary as supported_formats.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let resolution = Resolution::new(
            avf_format.resolution.width as u32,
            avf_format.resolution.height as u32,
        );
        let fourcc = avf_format.fourcc;

        // Prefer the device's actually-negotiated frame rate (set by set_all via
        // setActiveVideoMinFrameDuration) over the per-format maximum. Without
        // this, a format whose ranges span [1–30] and [1–60] always reports 60
        // fps even after the caller negotiated 30, causing Buffer metadata to
        // carry the wrong frame rate.
        //
        // activeVideoMinFrameDuration is a CMTime representing the minimum frame
        // delivery interval: fps = timescale / value (seconds per frame inverted).
        // An invalid CMTime (kCMTimeInvalid) means the property has not been set
        // explicitly, so we fall back to the format maximum.
        let min_dur = device_active_video_min_frame_duration(&self.inner);
        // min_dur.value is i64 but camera frame-count values (e.g. 1, 3) are
        // nowhere near the 2^52 mantissa boundary, so the cast is lossless in
        // practice.
        #[allow(clippy::cast_precision_loss)]
        let negotiated_fps: Option<f64> =
            if min_dur.flags.contains(CMTimeFlags::Valid) && min_dur.value > 0 {
                Some(f64::from(min_dur.timescale) / min_dur.value as f64)
            } else {
                None
            };

        // Match the negotiated fps to the nearest entry in fps_list (tolerance
        // mirrors set_all's < 0.999 comparison), or fall back to the maximum.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let fps: u32 = if let Some(neg_fps) = negotiated_fps {
            avf_format
                .fps_list
                .iter()
                .copied()
                .find(|&f| (f - neg_fps).abs() < 0.999)
                .unwrap_or_else(|| avf_format.fps_list.last().copied().unwrap_or(neg_fps))
                as u32
        } else {
            avf_format.fps_list.last().copied().unwrap_or(0.0) as u32
        };

        if fps == 0 {
            return Err(NokhwaError::get_property("activeFormat", "None??"));
        }

        Ok(CameraFormat::new(resolution, fourcc, fps))
    }
}

/// Standard `KnownCameraControlFlag` set for an `AVFoundation` control
/// based on whether the device reports it as supported. Unsupported
/// controls are surfaced to callers as `Disabled` + `ReadOnly` so the
/// `CameraControl` round-trips and `set_control` rejects writes
/// without an `unwrap` on missing platform support.
fn disabled_if_unsupported(supported: bool) -> Vec<KnownCameraControlFlag> {
    if supported {
        vec![]
    } else {
        vec![
            KnownCameraControlFlag::Disabled,
            KnownCameraControlFlag::ReadOnly,
        ]
    }
}

fn check_control_flags(
    ctrl: &CameraControl,
    id: &KnownCameraControl,
    value: &ControlValueSetter,
) -> Result<(), NokhwaError> {
    if ctrl.flag().contains(&KnownCameraControlFlag::ReadOnly) {
        return Err(NokhwaError::set_property(
            id.to_string(),
            value.to_string(),
            "Read Only",
        ));
    }
    if ctrl.flag().contains(&KnownCameraControlFlag::Disabled) {
        return Err(NokhwaError::set_property(
            id.to_string(),
            value.to_string(),
            "Disabled",
        ));
    }
    Ok(())
}

fn get_and_check_control<'a>(
    controls: &'a BTreeMap<KnownCameraControl, &'a CameraControl>,
    id: &KnownCameraControl,
    value: &ControlValueSetter,
) -> Result<&'a CameraControl, NokhwaError> {
    let ctrlvalue = controls.get(id).ok_or_else(|| {
        NokhwaError::set_property(id.to_string(), value.to_string(), "Control does not exist")
    })?;
    check_control_flags(ctrlvalue, id, value)?;
    Ok(ctrlvalue)
}

fn extract_float(value: &ControlValueSetter, id: &KnownCameraControl) -> Result<f64, NokhwaError> {
    value.as_float().copied().ok_or_else(|| {
        NokhwaError::set_property(id.to_string(), value.to_string(), "Expected float")
    })
}

fn extract_integer(
    value: &ControlValueSetter,
    id: &KnownCameraControl,
) -> Result<i64, NokhwaError> {
    value
        .as_integer()
        .copied()
        .ok_or_else(|| NokhwaError::set_property(id.to_string(), value.to_string(), "Expected i64"))
}

fn extract_enum(value: &ControlValueSetter, id: &KnownCameraControl) -> Result<i64, NokhwaError> {
    value.as_enum().copied().ok_or_else(|| {
        NokhwaError::set_property(id.to_string(), value.to_string(), "Expected Enum")
    })
}

fn extract_boolean(
    value: &ControlValueSetter,
    id: &KnownCameraControl,
) -> Result<bool, NokhwaError> {
    value.as_boolean().copied().ok_or_else(|| {
        NokhwaError::set_property(id.to_string(), value.to_string(), "Expected bool")
    })
}

fn verify_or_error(
    ctrl: &CameraControl,
    value: &ControlValueSetter,
    id: &KnownCameraControl,
) -> Result<(), NokhwaError> {
    if ctrl.description().verify_setter(value) {
        Ok(())
    } else {
        Err(NokhwaError::set_property(
            id.to_string(),
            value.to_string(),
            "Failed to verify value",
        ))
    }
}
