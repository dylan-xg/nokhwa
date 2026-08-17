use crate::error::NokhwaError;
use crate::format_types::CaptureFormat;
#[cfg(feature = "serialize")]
use serde::{Deserialize, Serialize};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    fmt::{Display, Formatter},
    str::FromStr,
};

/// Tells the init function what camera format to pick.
///
/// # Variants
///
/// | Variant | Behaviour |
/// |---------|-----------|
/// | `AbsoluteHighestResolution` | Pick the highest [`Resolution`], then the highest frame rate at that resolution. |
/// | `AbsoluteHighestFrameRate` | Pick the highest frame rate, then the highest [`Resolution`] at that rate. |
/// | `HighestResolution(Resolution)` | Given a specific [`Resolution`], pick the highest frame rate available at that resolution. |
/// | `HighestFrameRate(u32)` | Given a specific frame rate, pick the highest [`Resolution`] available at that rate. |
/// | `Exact(CameraFormat)` | Pick the exact [`CameraFormat`] provided, or fail. |
/// | `Closest(CameraFormat)` | Pick the closest match by [`FrameFormat`], then [`Resolution`], then FPS. Fails if the [`FrameFormat`] is unavailable. |
/// | `None` | Pick the first available format (default). |
#[derive(Copy, Clone, Debug, Default, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum RequestedFormatType {
    AbsoluteHighestResolution,
    AbsoluteHighestFrameRate,
    HighestResolution(Resolution),
    HighestFrameRate(u32),
    Exact(CameraFormat),
    Closest(CameraFormat),
    #[default]
    None,
}

impl Display for RequestedFormatType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A request to the camera for a valid [`CameraFormat`].
///
/// Combines a [`RequestedFormatType`] (what resolution/framerate strategy to use) with
/// a set of acceptable [`FrameFormat`]s (pixel formats the caller can decode).
///
/// # Examples
///
/// **Highest resolution (most common):**
///
/// ```ignore
/// use nokhwa_core::format_types::Mjpeg;
/// use nokhwa_core::types::{RequestedFormat, RequestedFormatType};
///
/// let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::AbsoluteHighestResolution);
/// ```
///
/// **Highest frame rate:**
///
/// ```ignore
/// use nokhwa_core::format_types::Mjpeg;
/// use nokhwa_core::types::{RequestedFormat, RequestedFormatType};
///
/// let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::AbsoluteHighestFrameRate);
/// ```
///
/// **Exact format:**
///
/// ```ignore
/// use nokhwa_core::format_types::Mjpeg;
/// use nokhwa_core::types::{
///     CameraFormat, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
/// };
///
/// let fmt = CameraFormat::new(Resolution::new(1920, 1080), FrameFormat::MJPEG, 30);
/// let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::Exact(fmt));
/// ```
///
/// **Closest match:**
///
/// ```ignore
/// use nokhwa_core::format_types::Mjpeg;
/// use nokhwa_core::types::{
///     CameraFormat, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
/// };
///
/// // Ask for 1080p@60; the library will find the closest the hardware supports.
/// let target = CameraFormat::new(Resolution::new(1920, 1080), FrameFormat::MJPEG, 60);
/// let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::Closest(target));
/// ```
///
/// **Best frame rate at a specific resolution (e.g. 1080p):**
///
/// ```ignore
/// use nokhwa_core::format_types::Mjpeg;
/// use nokhwa_core::types::{RequestedFormat, RequestedFormatType, Resolution};
///
/// // Find the highest frame rate available at 1920x1080
/// let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::HighestResolution(
///     Resolution::new(1920, 1080),
/// ));
/// ```
///
/// **Best resolution at a specific frame rate (e.g. 30 FPS):**
///
/// ```ignore
/// use nokhwa_core::format_types::Mjpeg;
/// use nokhwa_core::types::{RequestedFormat, RequestedFormatType};
///
/// // Find the highest resolution available at 30 FPS
/// let req = RequestedFormat::new::<Mjpeg>(RequestedFormatType::HighestFrameRate(30));
/// ```
///
/// **Custom frame format list:**
///
/// ```ignore
/// use nokhwa_core::types::{FrameFormat, RequestedFormat, RequestedFormatType};
///
/// let formats = &[FrameFormat::MJPEG, FrameFormat::YUYV];
/// let req = RequestedFormat::with_formats(
///     RequestedFormatType::AbsoluteHighestResolution,
///     formats,
/// );
/// ```
#[derive(Copy, Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct RequestedFormat<'a> {
    requested_format: RequestedFormatType,
    wanted_decoder: &'a [FrameFormat],
}

impl RequestedFormat<'_> {
    /// Creates a new [`RequestedFormat`] from a [`CaptureFormat`] type.
    ///
    /// The format constraint is derived from `F::FRAME_FORMAT`.
    #[must_use]
    pub fn new<F: CaptureFormat>(requested: RequestedFormatType) -> RequestedFormat<'static> {
        // FRAME_FORMAT is a const, so &[F::FRAME_FORMAT] is promoted to static memory
        // by the compiler (constant promotion). No heap allocation occurs.
        let formats: &'static [FrameFormat] = &[F::FRAME_FORMAT];
        RequestedFormat {
            requested_format: requested,
            wanted_decoder: formats,
        }
    }

    /// Creates a new [`RequestedFormat`] by using the [`RequestedFormatType`] and getting the [`FrameFormat`]
    /// constraints from a statically allocated slice.
    #[must_use]
    pub fn with_formats(
        requested: RequestedFormatType,
        decoder: &[FrameFormat],
    ) -> RequestedFormat<'_> {
        RequestedFormat {
            requested_format: requested,
            wanted_decoder: decoder,
        }
    }

    /// Gets the [`RequestedFormatType`]
    #[must_use]
    pub fn requested_format_type(&self) -> RequestedFormatType {
        self.requested_format
    }

    /// Fulfill the requested using a list of all available formats.
    ///
    /// See [`RequestedFormatType`] for more details.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn fulfill(&self, all_formats: &[CameraFormat]) -> Option<CameraFormat> {
        match self.requested_format {
            RequestedFormatType::AbsoluteHighestResolution => {
                let max_resolution = all_formats
                    .iter()
                    .filter(|fmt| self.wanted_decoder.contains(&fmt.format()))
                    .max_by_key(|fmt| fmt.resolution())?
                    .resolution();
                all_formats
                    .iter()
                    .filter(|fmt| {
                        fmt.resolution() == max_resolution
                            && self.wanted_decoder.contains(&fmt.format())
                    })
                    .max_by_key(|fmt| fmt.frame_rate())
                    .copied()
            },
            RequestedFormatType::AbsoluteHighestFrameRate => {
                let max_frame_rate = all_formats
                    .iter()
                    .filter(|fmt| self.wanted_decoder.contains(&fmt.format()))
                    .max_by_key(|fmt| fmt.frame_rate())?
                    .frame_rate();
                all_formats
                    .iter()
                    .filter(|fmt| {
                        fmt.frame_rate() == max_frame_rate
                            && self.wanted_decoder.contains(&fmt.format())
                    })
                    .max_by_key(|fmt| fmt.resolution())
                    .copied()
            },
            RequestedFormatType::HighestResolution(res) => {
                let highest_fps = all_formats
                    .iter()
                    .filter(|x| x.resolution == res && self.wanted_decoder.contains(&x.format()))
                    .max_by_key(|x| x.frame_rate)?
                    .frame_rate;
                all_formats
                    .iter()
                    .filter(|x| {
                        x.resolution == res
                            && x.frame_rate == highest_fps
                            && self.wanted_decoder.contains(&x.format())
                    })
                    .max_by_key(|x| x.format())
                    .copied()
            },
            RequestedFormatType::HighestFrameRate(fps) => {
                let highest_res = all_formats
                    .iter()
                    .filter(|x| x.frame_rate == fps && self.wanted_decoder.contains(&x.format()))
                    .max_by_key(|x| x.resolution)?
                    .resolution;
                all_formats
                    .iter()
                    .filter(|x| {
                        x.frame_rate == fps
                            && x.resolution() == highest_res
                            && self.wanted_decoder.contains(&x.format())
                    })
                    .max_by_key(|x| x.format())
                    .copied()
            },
            RequestedFormatType::Exact(fmt) => {
                if self.wanted_decoder.contains(&fmt.format()) {
                    Some(fmt)
                } else {
                    None
                }
            },
            #[allow(clippy::cast_possible_wrap)]
            RequestedFormatType::Closest(c) => {
                let same_fmt_formats = all_formats
                    .iter()
                    .filter(|x| {
                        x.format() == c.format() && self.wanted_decoder.contains(&x.format())
                    })
                    .copied()
                    .collect::<Vec<CameraFormat>>();
                let mut resolution_map = same_fmt_formats
                    .iter()
                    .map(|x| {
                        let res = x.resolution();
                        let x_diff = res.x() as i32 - c.resolution().x() as i32;
                        let y_diff = res.y() as i32 - c.resolution().y() as i32;
                        let dist_no_sqrt = x_diff.abs().pow(2) + y_diff.abs().pow(2);
                        (dist_no_sqrt, res)
                    })
                    .collect::<Vec<(i32, Resolution)>>();
                resolution_map.sort_by_key(|a| a.0);
                resolution_map.dedup_by(|a, b| a.0.eq(&b.0));
                let resolution = resolution_map.first()?.1;

                let frame_rates = all_formats
                    .iter()
                    .filter_map(|cfmt| {
                        if cfmt.format() == c.format() && cfmt.resolution() == resolution {
                            return Some(cfmt.frame_rate());
                        }
                        None
                    })
                    .collect::<Vec<u32>>();
                // sort FPSes
                let mut framerate_map = frame_rates
                    .iter()
                    .map(|x| {
                        let abs = *x as i32 - c.frame_rate() as i32;
                        (abs.unsigned_abs(), *x)
                    })
                    .collect::<Vec<(u32, u32)>>();
                framerate_map.sort_by_key(|a| a.0);
                let frame_rate = framerate_map.first()?.1;
                Some(CameraFormat::new(resolution, c.format(), frame_rate))
            },
            RequestedFormatType::None => all_formats
                .iter()
                .find(|fmt| self.wanted_decoder.contains(&fmt.format()))
                .copied(),
        }
    }
}

impl Display for RequestedFormat<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Describes the index of the camera.
/// - Index: A numbered index
/// - String: A string, used for `IPCameras`.
#[derive(Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum CameraIndex {
    Index(u32),
    String(String),
}

impl CameraIndex {
    /// Turns this value into a number. If it is a string, it will attempt to parse it as a `u32`.
    /// # Errors
    /// Fails if the value is not a number.
    pub fn as_index(&self) -> Result<u32, NokhwaError> {
        match self {
            CameraIndex::Index(i) => Ok(*i),
            CameraIndex::String(s) => s
                .parse::<u32>()
                .map_err(|why| NokhwaError::general(why.to_string())),
        }
    }

    /// Turns this value into a `String`. If it is a number, it will be automatically converted.
    #[must_use]
    pub fn as_string(&self) -> String {
        match self {
            CameraIndex::Index(i) => i.to_string(),
            CameraIndex::String(s) => s.clone(),
        }
    }

    /// Returns true if this [`CameraIndex`] contains an [`CameraIndex::Index`]
    #[must_use]
    pub fn is_index(&self) -> bool {
        match self {
            CameraIndex::Index(_) => true,
            CameraIndex::String(_) => false,
        }
    }

    /// Returns true if this [`CameraIndex`] contains an [`CameraIndex::String`]
    #[must_use]
    pub fn is_string(&self) -> bool {
        !self.is_index()
    }
}

impl Display for CameraIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_string())
    }
}

impl Default for CameraIndex {
    fn default() -> Self {
        CameraIndex::Index(0)
    }
}

impl TryFrom<CameraIndex> for u32 {
    type Error = NokhwaError;

    fn try_from(value: CameraIndex) -> Result<Self, Self::Error> {
        value.as_index()
    }
}

impl TryFrom<CameraIndex> for usize {
    type Error = NokhwaError;

    fn try_from(value: CameraIndex) -> Result<Self, Self::Error> {
        value.as_index().map(|i| i as usize)
    }
}

/// Describes a frame format (i.e. how the bytes themselves are encoded). Often called `FourCC`.
/// - YUYV is a mathematical color space. You can read more [here.](https://en.wikipedia.org/wiki/YCbCr)
/// - NV12 is same as above. Note that a partial compression (e.g. [16, 235] may be coerced to [0, 255].
/// - MJPEG is a motion-jpeg compressed frame, it allows for high frame rates.
/// - GRAY is a grayscale image format, usually for specialized cameras such as IR Cameras.
/// - RAWRGB is a Raw RGB888 format.
#[derive(Copy, Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum FrameFormat {
    MJPEG,
    YUYV,
    NV12,
    GRAY,
    RAWRGB,
    RAWBGR,
}

impl Display for FrameFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameFormat::MJPEG => {
                write!(f, "MJPEG")
            },
            FrameFormat::YUYV => {
                write!(f, "YUYV")
            },
            FrameFormat::GRAY => {
                write!(f, "GRAY")
            },
            FrameFormat::RAWRGB => {
                write!(f, "RAWRGB")
            },
            FrameFormat::RAWBGR => {
                write!(f, "RAWBGR")
            },
            FrameFormat::NV12 => {
                write!(f, "NV12")
            },
        }
    }
}
impl FromStr for FrameFormat {
    type Err = NokhwaError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "MJPEG" => Ok(FrameFormat::MJPEG),
            "YUYV" => Ok(FrameFormat::YUYV),
            "GRAY" => Ok(FrameFormat::GRAY),
            "RAWRGB" => Ok(FrameFormat::RAWRGB),
            "RAWBGR" => Ok(FrameFormat::RAWBGR),
            "NV12" => Ok(FrameFormat::NV12),
            _ => Err(NokhwaError::structure(
                "FrameFormat",
                format!("No match for {s}"),
            )),
        }
    }
}

impl FrameFormat {
    /// Converts a `FourCC` string (e.g. `"YUYV"`, `"MJPG"`) into a [`FrameFormat`].
    ///
    /// This centralises the canonical FourCC-to-frame-format mapping so that
    /// platform backends do not each have to duplicate the same table.
    #[must_use]
    pub fn from_fourcc(fourcc: &str) -> Option<Self> {
        match fourcc {
            "YUYV" => Some(Self::YUYV),
            "MJPG" => Some(Self::MJPEG),
            "GRAY" => Some(Self::GRAY),
            "RGB3" => Some(Self::RAWRGB),
            "BGR3" => Some(Self::RAWBGR),
            "NV12" => Some(Self::NV12),
            _ => None,
        }
    }

    /// Returns the canonical `FourCC` string for this frame format.
    #[must_use]
    pub const fn to_fourcc(&self) -> &'static str {
        match self {
            Self::MJPEG => "MJPG",
            Self::YUYV => "YUYV",
            Self::GRAY => "GRAY",
            Self::RAWRGB => "RGB3",
            Self::RAWBGR => "BGR3",
            Self::NV12 => "NV12",
        }
    }

    /// Bytes per pixel after **decoding** to a flat RGB-style or grayscale
    /// surface — `1` for [`FrameFormat::GRAY`], `3` for every color format.
    /// This is *not* the wire size: e.g. YUYV is 2 bytes/pixel on the wire,
    /// NV12 is 1.5, MJPEG is variable. Use this when sizing a destination
    /// buffer for a decoded frame.
    ///
    /// Centralises the table that was previously duplicated in
    /// `FrameSource::decoded_buffer_size` and the test-fixture
    /// `mock_frame()`.
    #[must_use]
    pub const fn decoded_pixel_byte_width(&self) -> usize {
        match self {
            Self::MJPEG | Self::YUYV | Self::RAWRGB | Self::RAWBGR | Self::NV12 => 3,
            Self::GRAY => 1,
        }
    }
}

/// Returns all the frame formats
#[must_use]
pub const fn frame_formats() -> &'static [FrameFormat] {
    &[
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::GRAY,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
    ]
}

/// Returns all the color frame formats
#[must_use]
pub const fn color_frame_formats() -> &'static [FrameFormat] {
    &[
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::RAWRGB,
        FrameFormat::RAWBGR,
    ]
}

/// Describes a Resolution.
/// This struct consists of a Width and a Height value (x,y). <br>
/// Note: the [`Ord`] implementation is **lexicographic and ascending**:
/// width is the primary key (lowest first), height is the tiebreaker
/// (lowest first). `Vec::iter().max()` therefore returns the largest
/// resolution, which is what [`RequestedFormat::fulfill`] relies on.
/// # JS-WASM
/// This is exported as `JSResolution`
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct Resolution {
    pub width_x: u32,
    pub height_y: u32,
}

impl Resolution {
    /// Create a new resolution from 2 image size coordinates.
    /// # JS-WASM
    /// This is exported as a constructor for [`Resolution`].
    #[must_use]
    pub fn new(x: u32, y: u32) -> Self {
        Resolution {
            width_x: x,
            height_y: y,
        }
    }

    /// Get the width of Resolution
    /// # JS-WASM
    /// This is exported as `get_Width`.
    #[must_use]
    #[inline]
    pub fn width(self) -> u32 {
        self.width_x
    }

    /// Get the height of Resolution
    /// # JS-WASM
    /// This is exported as `get_Height`.
    #[must_use]
    #[inline]
    pub fn height(self) -> u32 {
        self.height_y
    }

    /// Get the x (width) of Resolution
    #[must_use]
    #[inline]
    pub fn x(self) -> u32 {
        self.width_x
    }

    /// Get the y (height) of Resolution
    #[must_use]
    #[inline]
    pub fn y(self) -> u32 {
        self.height_y
    }
}

impl Display for Resolution {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.x(), self.y())
    }
}

impl PartialOrd for Resolution {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Resolution {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.x().cmp(&other.x()) {
            Ordering::Less => Ordering::Less,
            Ordering::Equal => self.y().cmp(&other.y()),
            Ordering::Greater => Ordering::Greater,
        }
    }
}

/// This is a convenience struct that holds all information about the format of a webcam stream.
/// It consists of a [`Resolution`], [`FrameFormat`], and a frame rate(u8).
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub struct CameraFormat {
    resolution: Resolution,
    format: FrameFormat,
    frame_rate: u32,
}

impl CameraFormat {
    /// Construct a new [`CameraFormat`]
    #[must_use]
    pub fn new(resolution: Resolution, format: FrameFormat, frame_rate: u32) -> Self {
        CameraFormat {
            resolution,
            format,
            frame_rate,
        }
    }

    /// [`CameraFormat::new()`], but raw.
    #[must_use]
    pub fn new_from(res_x: u32, res_y: u32, format: FrameFormat, fps: u32) -> Self {
        CameraFormat {
            resolution: Resolution {
                width_x: res_x,
                height_y: res_y,
            },
            format,
            frame_rate: fps,
        }
    }

    /// Get the resolution of the current [`CameraFormat`]
    #[must_use]
    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    /// Get the width of the resolution of the current [`CameraFormat`]
    #[must_use]
    pub fn width(&self) -> u32 {
        self.resolution.width()
    }

    /// Get the height of the resolution of the current [`CameraFormat`]
    #[must_use]
    pub fn height(&self) -> u32 {
        self.resolution.height()
    }

    /// Set the [`CameraFormat`]'s resolution.
    pub fn set_resolution(&mut self, resolution: Resolution) {
        self.resolution = resolution;
    }

    /// Get the frame rate of the current [`CameraFormat`]
    #[must_use]
    pub fn frame_rate(&self) -> u32 {
        self.frame_rate
    }

    /// Set the [`CameraFormat`]'s frame rate.
    pub fn set_frame_rate(&mut self, frame_rate: u32) {
        self.frame_rate = frame_rate;
    }

    /// Get the [`CameraFormat`]'s format.
    #[must_use]
    pub fn format(&self) -> FrameFormat {
        self.format
    }

    /// Set the [`CameraFormat`]'s format.
    pub fn set_format(&mut self, format: FrameFormat) {
        self.format = format;
    }
}

impl Default for CameraFormat {
    fn default() -> Self {
        CameraFormat {
            resolution: Resolution::new(640, 480),
            format: FrameFormat::MJPEG,
            frame_rate: 30,
        }
    }
}

impl Display for CameraFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}@{}FPS, {} Format",
            self.resolution, self.frame_rate, self.format
        )
    }
}

/// Information about a Camera e.g. its name.
/// `description` amd `misc` may contain information that may differ from backend to backend. Refer to each backend for details.
/// `index` is a camera's index given to it by (usually) the OS usually in the order it is known to the system.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub struct CameraInfo {
    human_name: String,
    description: String,
    misc: String,
    index: CameraIndex,
}

impl CameraInfo {
    /// Create a new [`CameraInfo`].
    /// # JS-WASM
    /// This is exported as a constructor for [`CameraInfo`].
    #[must_use]
    // OK, i just checkeed back on this code. WTF was I on when I wrote `&(impl AsRef<str> + ?Sized)` ????
    // I need to get on the same shit that my previous self was on, because holy shit that stuff is strong as FUCK!
    // Finally fixed this insanity. Hopefully I didn't torment anyone by actually putting this in a stable release.
    pub fn new(human_name: &str, description: &str, misc: &str, index: CameraIndex) -> Self {
        CameraInfo {
            human_name: human_name.to_string(),
            description: description.to_string(),
            misc: misc.to_string(),
            index,
        }
    }

    /// Get a reference to the device info's human readable name.
    /// # JS-WASM
    /// This is exported as a `get_HumanReadableName`.
    #[must_use]
    // yes, i know, unnecessary alloc this, unnecessary alloc that
    // but wasm bindgen
    pub fn human_name(&self) -> String {
        self.human_name.clone()
    }

    /// Set the device info's human name.
    /// # JS-WASM
    /// This is exported as a `set_HumanReadableName`.
    pub fn set_human_name(&mut self, human_name: &str) {
        self.human_name = human_name.to_string();
    }

    /// Get a reference to the device info's description.
    /// # JS-WASM
    /// This is exported as a `get_Description`.
    #[must_use]
    pub fn description(&self) -> &str {
        self.description.borrow()
    }

    /// Set the device info's description.
    /// # JS-WASM
    /// This is exported as a `set_Description`.
    pub fn set_description(&mut self, description: &str) {
        self.description = description.to_string();
    }

    /// Get a reference to the device info's misc.
    /// # JS-WASM
    /// This is exported as a `get_MiscString`.
    #[must_use]
    pub fn misc(&self) -> String {
        self.misc.clone()
    }

    /// Set the device info's misc.
    /// # JS-WASM
    /// This is exported as a `set_MiscString`.
    pub fn set_misc(&mut self, misc: &str) {
        self.misc = misc.to_string();
    }

    /// Get a reference to the device info's index.
    /// # JS-WASM
    /// This is exported as a `get_Index`.
    #[must_use]
    pub fn index(&self) -> &CameraIndex {
        &self.index
    }

    /// Set the device info's index.
    /// # JS-WASM
    /// This is exported as a `set_Index`.
    pub fn set_index(&mut self, index: CameraIndex) {
        self.index = index;
    }
}

impl Display for CameraInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Name: {}, Description: {}, Extra: {}, Index: {}",
            self.human_name, self.description, self.misc, self.index
        )
    }
}

/// The list of known camera controls to the library. <br>
/// These can control the picture brightness, etc. <br>
/// Note that not all backends/devices support all these. Call
/// [`CameraDevice::controls()`](crate::traits::CameraDevice::controls) to see which
/// ones are reported by a given backend.
#[derive(Copy, Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum KnownCameraControl {
    Brightness,
    Contrast,
    Hue,
    Saturation,
    Sharpness,
    Gamma,
    WhiteBalance,
    BacklightComp,
    Gain,
    Pan,
    Tilt,
    Zoom,
    Exposure,
    Iris,
    Focus,
    AutoExposure,
    ExposureAbsolute,
    AutoWhiteBalance,
    PowerLineFrequency,
    ExposureAutoPriority,
    /// Other camera control. Listed is the ID.
    /// Wasteful, however is needed for a unified API across Windows, Linux, and `MacOSX` due to Microsoft's usage of `GUIDs`.
    ///
    /// THIS SHOULD ONLY BE USED WHEN YOU KNOW THE PLATFORM THAT YOU ARE RUNNING ON.
    Other(u128),
}

/// All camera controls in an array.
#[must_use]
pub const fn all_known_camera_controls() -> [KnownCameraControl; KnownCameraControl::STANDARD_COUNT]
{
    [
        KnownCameraControl::Brightness,
        KnownCameraControl::Contrast,
        KnownCameraControl::Hue,
        KnownCameraControl::Saturation,
        KnownCameraControl::Sharpness,
        KnownCameraControl::Gamma,
        KnownCameraControl::WhiteBalance,
        KnownCameraControl::BacklightComp,
        KnownCameraControl::Gain,
        KnownCameraControl::Pan,
        KnownCameraControl::Tilt,
        KnownCameraControl::Zoom,
        KnownCameraControl::Exposure,
        KnownCameraControl::Iris,
        KnownCameraControl::Focus,
        KnownCameraControl::AutoExposure,
        KnownCameraControl::ExposureAbsolute,
        KnownCameraControl::AutoWhiteBalance,
        KnownCameraControl::PowerLineFrequency,
        KnownCameraControl::ExposureAutoPriority,
    ]
}

impl KnownCameraControl {
    /// Returns a canonical zero-based index for each standard control.
    ///
    /// Platform backends can use this together with [`from_index`](Self::from_index)
    /// and a platform-specific ID table to avoid duplicating the full match arms.
    /// Returns `None` for the `Other` variant.
    #[must_use]
    pub const fn as_index(&self) -> Option<u8> {
        match self {
            Self::Brightness => Some(0),
            Self::Contrast => Some(1),
            Self::Hue => Some(2),
            Self::Saturation => Some(3),
            Self::Sharpness => Some(4),
            Self::Gamma => Some(5),
            Self::WhiteBalance => Some(6),
            Self::BacklightComp => Some(7),
            Self::Gain => Some(8),
            Self::Pan => Some(9),
            Self::Tilt => Some(10),
            Self::Zoom => Some(11),
            Self::Exposure => Some(12),
            Self::Iris => Some(13),
            Self::Focus => Some(14),
            Self::AutoExposure => Some(15),
            Self::ExposureAbsolute => Some(16),
            Self::AutoWhiteBalance => Some(17),
            Self::PowerLineFrequency => Some(18),
            Self::ExposureAutoPriority => Some(19),
            Self::Other(_) => None,
        }
    }

    /// Converts a canonical zero-based index back into a [`KnownCameraControl`].
    ///
    /// The index values correspond to those returned by [`as_index`](Self::as_index).
    #[must_use]
    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Brightness),
            1 => Some(Self::Contrast),
            2 => Some(Self::Hue),
            3 => Some(Self::Saturation),
            4 => Some(Self::Sharpness),
            5 => Some(Self::Gamma),
            6 => Some(Self::WhiteBalance),
            7 => Some(Self::BacklightComp),
            8 => Some(Self::Gain),
            9 => Some(Self::Pan),
            10 => Some(Self::Tilt),
            11 => Some(Self::Zoom),
            12 => Some(Self::Exposure),
            13 => Some(Self::Iris),
            14 => Some(Self::Focus),
            15 => Some(Self::AutoExposure),
            16 => Some(Self::ExposureAbsolute),
            17 => Some(Self::AutoWhiteBalance),
            18 => Some(Self::PowerLineFrequency),
            19 => Some(Self::ExposureAutoPriority),
            _ => None,
        }
    }

    /// Number of standard (non-`Other`) controls.
    pub const STANDARD_COUNT: usize = 20;

    /// Look up a [`KnownCameraControl`] from a platform-specific ID using a
    /// table that maps canonical indices to platform IDs.
    ///
    /// `platform_ids` must have exactly [`STANDARD_COUNT`](Self::STANDARD_COUNT)
    /// entries, one per canonical index.
    ///
    /// **Note:** This helper uses `u32` platform IDs, which is suitable for
    /// V4L2 CIDs.  Windows Media Foundation uses GUIDs that do not fit in
    /// `u32`; MSMF should use its own mapping rather than this function.
    #[must_use]
    pub fn from_platform_id(platform_id: u32, platform_ids: &[u32; Self::STANDARD_COUNT]) -> Self {
        for (idx, &pid) in platform_ids.iter().enumerate() {
            if pid == platform_id {
                // idx is always 0..14 because STANDARD_COUNT == 15, so this cast is safe.
                #[allow(clippy::cast_possible_truncation)]
                if let Some(ctrl) = Self::from_index(idx as u8) {
                    return ctrl;
                }
            }
        }
        KnownCameraControl::Other(u128::from(platform_id))
    }

    /// Convert a [`KnownCameraControl`] to a platform-specific ID using a
    /// table that maps canonical indices to platform IDs.
    ///
    /// For standard controls the corresponding entry in `platform_ids` is
    /// returned.  For `Other(id)` the stored value is truncated to `u32`.
    ///
    /// **Note:** This helper uses `u32` platform IDs, which is suitable for
    /// V4L2 CIDs.  Windows Media Foundation uses GUIDs that do not fit in
    /// `u32`; MSMF should use its own mapping rather than this function.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn to_platform_id(&self, platform_ids: &[u32; Self::STANDARD_COUNT]) -> u32 {
        match self.as_index() {
            Some(i) => platform_ids[i as usize],
            None => match self {
                Self::Other(id) => *id as u32,
                _ => 0, // unreachable: as_index returns Some for all non-Other variants
            },
        }
    }
}

impl Display for KnownCameraControl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// This tells you weather a [`KnownCameraControl`] is automatically managed by the OS/Driver
/// or manually managed by you, the programmer.
#[derive(Copy, Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum KnownCameraControlFlag {
    Automatic,
    Manual,
    Continuous,
    ReadOnly,
    WriteOnly,
    Volatile,
    Disabled,
}

impl Display for KnownCameraControlFlag {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// The different types of menu items that can appear in [`ControlValueDescription::Menu`].
#[derive(Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum MenuItem {
    Name(String),
    Value(i64),
}

impl From<String> for MenuItem {
    fn from(value: String) -> Self {
        Self::Name(value)
    }
}

impl From<i64> for MenuItem {
    fn from(value: i64) -> Self {
        Self::Value(value)
    }
}

impl Display for MenuItem {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MenuItem::Name(s) => write!(f, "Name: {}", s),
            MenuItem::Value(i) => write!(f, "Value: {}", i),
        }
    }
}

/// A single entry into [`ControlValueDescription::Menu`].
#[derive(Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub struct MenuEntry {
    id: u32,
    item: MenuItem,
}

impl MenuEntry {
    pub fn new(id: u32, item: MenuItem) -> Self {
        Self { id, item }
    }

    pub fn get_id(&self) -> u32 {
        self.id
    }

    pub fn get_item(&self) -> MenuItem {
        self.item.clone()
    }
}

impl Display for MenuEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Id: {}, {}", self.id, self.item)
    }
}

/// The values for a [`CameraControl`].
///
/// This provides a wide range of values that can be used to control a camera.
#[derive(Clone, Debug, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum ControlValueDescription {
    None,
    Integer {
        value: i64,
        default: i64,
        step: i64,
    },
    IntegerRange {
        min: i64,
        max: i64,
        value: i64,
        step: i64,
        default: i64,
    },
    Float {
        value: f64,
        default: f64,
        step: f64,
    },
    FloatRange {
        min: f64,
        max: f64,
        value: f64,
        step: f64,
        default: f64,
    },
    Boolean {
        value: bool,
        default: bool,
    },
    String {
        value: String,
        default: Option<String>,
    },
    Bytes {
        value: Vec<u8>,
        default: Vec<u8>,
    },
    KeyValuePair {
        key: i128,
        value: i128,
        default: (i128, i128),
    },
    Point {
        value: (f64, f64),
        default: (f64, f64),
    },
    Enum {
        value: i64,
        possible: Vec<i64>,
        default: i64,
    },
    RGB {
        value: (f64, f64, f64),
        max: (f64, f64, f64),
        default: (f64, f64, f64),
    },
    Menu {
        value: u32,
        items: Vec<MenuEntry>,
        default: u32,
    },
}

impl ControlValueDescription {
    /// Get the value of this [`ControlValueDescription`]
    #[must_use]
    pub fn value(&self) -> ControlValueSetter {
        match self {
            ControlValueDescription::None => ControlValueSetter::None,
            ControlValueDescription::Integer { value, .. }
            | ControlValueDescription::IntegerRange { value, .. } => {
                ControlValueSetter::Integer(*value)
            },
            ControlValueDescription::Float { value, .. }
            | ControlValueDescription::FloatRange { value, .. } => {
                ControlValueSetter::Float(*value)
            },
            ControlValueDescription::Boolean { value, .. } => ControlValueSetter::Boolean(*value),
            ControlValueDescription::String { value, .. } => {
                ControlValueSetter::String(value.clone())
            },
            ControlValueDescription::Bytes { value, .. } => {
                ControlValueSetter::Bytes(value.clone())
            },
            ControlValueDescription::KeyValuePair { key, value, .. } => {
                ControlValueSetter::KeyValue(*key, *value)
            },
            ControlValueDescription::Point { value, .. } => {
                ControlValueSetter::Point(value.0, value.1)
            },
            ControlValueDescription::Enum { value, .. } => ControlValueSetter::EnumValue(*value),
            ControlValueDescription::RGB { value, .. } => {
                ControlValueSetter::RGB(value.0, value.1, value.2)
            },
            ControlValueDescription::Menu { value, .. } => ControlValueSetter::Menu(*value),
        }
    }

    /// Verifies if the [setter](ControlValueSetter) is valid for the provided [`ControlValueDescription`].
    /// - `true` => Is valid.
    /// - `false` => Is not valid.
    ///
    /// If the step is 0, it will automatically return `true`.
    #[must_use]
    pub fn verify_setter(&self, setter: &ControlValueSetter) -> bool {
        match self {
            ControlValueDescription::None => setter.as_none().is_some(),
            ControlValueDescription::Integer {
                value,
                default,
                step,
            } => {
                if *step == 0 {
                    return true;
                }
                match setter.as_integer() {
                    Some(i) => (i - default) % step == 0 || (i - value) % step == 0,
                    None => false,
                }
            },
            ControlValueDescription::IntegerRange {
                min,
                max,
                value,
                step,
                default,
            } => {
                if *step == 0 {
                    return true;
                }
                match setter.as_integer() {
                    Some(i) => {
                        ((i - default) % step == 0 || (i - value) % step == 0)
                            && i >= min
                            && i <= max
                    },
                    None => false,
                }
            },
            ControlValueDescription::Float {
                value,
                default,
                step,
            } => {
                if step.abs() == 0_f64 {
                    return true;
                }
                match setter.as_float() {
                    Some(f) => (f - default).abs() % step == 0_f64 || (f - value) % step == 0_f64,
                    None => false,
                }
            },
            ControlValueDescription::FloatRange {
                min,
                max,
                value,
                step,
                default,
            } => {
                if step.abs() == 0_f64 {
                    return true;
                }

                match setter.as_float() {
                    Some(f) => {
                        ((f - default).abs() % step == 0_f64 || (f - value) % step == 0_f64)
                            && f >= min
                            && f <= max
                    },
                    None => false,
                }
            },
            ControlValueDescription::Boolean { .. } => setter.as_boolean().is_some(),
            ControlValueDescription::String { .. } => setter.as_str().is_some(),
            ControlValueDescription::Bytes { .. } => setter.as_bytes().is_some(),
            ControlValueDescription::KeyValuePair { .. } => setter.as_key_value().is_some(),
            ControlValueDescription::Point { .. } => match setter.as_point() {
                Some(pt) => {
                    !pt.0.is_nan() && !pt.1.is_nan() && pt.0.is_finite() && pt.1.is_finite()
                },
                None => false,
            },
            ControlValueDescription::Enum { possible, .. } => match setter.as_enum() {
                Some(e) => possible.contains(e),
                None => false,
            },
            ControlValueDescription::RGB { max, .. } => match setter.as_rgb() {
                // Each channel must be a finite value within `0.0 ..= max`.
                // The previous predicate was `>= max` which only accepted
                // values *at or above* the upper bound, the inverse of
                // what range-validation should do — every other range
                // variant in this match (Integer/Float Range) uses
                // `value >= min && value <= max`.
                Some(v) => {
                    let in_range = |x: f64, lim: f64| x.is_finite() && x >= 0.0 && x <= lim;
                    in_range(*v.0, max.0) && in_range(*v.1, max.1) && in_range(*v.2, max.2)
                },
                None => false,
            },
            ControlValueDescription::Menu { items, .. } => match setter.as_menu() {
                Some(u) => items.iter().any(|entry| entry.id == *u),
                None => false,
            },
        }
    }
}

impl Display for ControlValueDescription {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlValueDescription::None => {
                write!(f, "(None)")
            },
            ControlValueDescription::Integer {
                value,
                default,
                step,
            } => {
                write!(f, "(Current: {value}, Default: {default}, Step: {step})")
            },
            ControlValueDescription::IntegerRange {
                min,
                max,
                value,
                step,
                default,
            } => {
                write!(
                    f,
                    "(Current: {value}, Default: {default}, Step: {step}, Range: ({min}, {max}))",
                )
            },
            ControlValueDescription::Float {
                value,
                default,
                step,
            } => {
                write!(f, "(Current: {value}, Default: {default}, Step: {step})")
            },
            ControlValueDescription::FloatRange {
                min,
                max,
                value,
                step,
                default,
            } => {
                write!(
                    f,
                    "(Current: {value}, Default: {default}, Step: {step}, Range: ({min}, {max}))",
                )
            },
            ControlValueDescription::Boolean { value, default } => {
                write!(f, "(Current: {value}, Default: {default})")
            },
            ControlValueDescription::String { value, default } => {
                write!(f, "(Current: {value}, Default: {default:?})")
            },
            ControlValueDescription::Bytes { value, default } => {
                write!(f, "(Current: {value:x?}, Default: {default:x?})")
            },
            ControlValueDescription::KeyValuePair {
                key,
                value,
                default,
            } => {
                write!(
                    f,
                    "Current: ({key}, {value}), Default: ({}, {})",
                    default.0, default.1
                )
            },
            ControlValueDescription::Point { value, default } => {
                write!(
                    f,
                    "Current: ({}, {}), Default: ({}, {})",
                    value.0, value.1, default.0, default.1
                )
            },
            ControlValueDescription::Enum {
                value,
                possible,
                default,
            } => {
                write!(
                    f,
                    "Current: {value}, Possible Values: {possible:?}, Default: {default}",
                )
            },
            ControlValueDescription::RGB {
                value,
                max,
                default,
            } => {
                write!(
                    f,
                    "Current: ({}, {}, {}), Max: ({}, {}, {}), Default: ({}, {}, {})",
                    value.0, value.1, value.2, max.0, max.1, max.2, default.0, default.1, default.2
                )
            },
            ControlValueDescription::Menu {
                value,
                items,
                default,
            } => {
                write!(
                    f,
                    "Current: {}, Entries: ({:?}), Default: {}",
                    value, items, default
                )
            },
        }
    }
}

/// This struct tells you everything about a particular [`KnownCameraControl`].
///
/// However, you should never need to instantiate this struct, since its usually generated for you by `nokhwa`.
/// The only time you should be modifying this struct is when you need to set a value and pass it back to the camera.
/// NOTE: Assume the values for `min` and `max` as **non-inclusive**!.
/// E.g. if the [`CameraControl`] says `min` is 100, the minimum is actually 101.
#[derive(Clone, Debug, PartialOrd, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub struct CameraControl {
    control: KnownCameraControl,
    name: String,
    description: ControlValueDescription,
    flag: Vec<KnownCameraControlFlag>,
    active: bool,
}

impl CameraControl {
    /// Creates a new [`CameraControl`]
    #[must_use]
    pub fn new(
        control: KnownCameraControl,
        name: String,
        description: ControlValueDescription,
        flag: Vec<KnownCameraControlFlag>,
        active: bool,
    ) -> Self {
        CameraControl {
            control,
            name,
            description,
            flag,
            active,
        }
    }

    /// Gets the name of this [`CameraControl`]
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gets the [`ControlValueDescription`] of this [`CameraControl`]
    #[must_use]
    pub fn description(&self) -> &ControlValueDescription {
        &self.description
    }

    /// Gets the [`ControlValueSetter`] of the [`ControlValueDescription`] of this [`CameraControl`]
    #[must_use]
    pub fn value(&self) -> ControlValueSetter {
        self.description.value()
    }

    /// Gets the [`KnownCameraControl`] of this [`CameraControl`]
    #[must_use]
    pub fn control(&self) -> KnownCameraControl {
        self.control
    }

    /// Gets the [`KnownCameraControlFlag`] of this [`CameraControl`],
    /// telling you weather this control is automatically set or manually set.
    #[must_use]
    pub fn flag(&self) -> &[KnownCameraControlFlag] {
        &self.flag
    }

    /// Gets `active` of this [`CameraControl`],
    /// telling you weather this control is currently active(in-use).
    #[must_use]
    pub fn active(&self) -> bool {
        self.active
    }

    /// Gets `active` of this [`CameraControl`],
    /// telling you weather this control is currently active(in-use).
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }
}

impl Display for CameraControl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Control: {}, Name: {}, Value: {}, Flag: {:?}, Active: {}",
            self.control, self.name, self.description, self.flag, self.active
        )
    }
}

/// The setter for a control value
#[derive(Clone, Debug, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum ControlValueSetter {
    None,
    Integer(i64),
    Float(f64),
    Boolean(bool),
    String(String),
    Bytes(Vec<u8>),
    KeyValue(i128, i128),
    Point(f64, f64),
    EnumValue(i64),
    RGB(f64, f64, f64),
    Menu(u32),
}

impl ControlValueSetter {
    #[must_use]
    pub fn as_none(&self) -> Option<()> {
        if let ControlValueSetter::None = self {
            Some(())
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_integer(&self) -> Option<&i64> {
        if let ControlValueSetter::Integer(i) = self {
            Some(i)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_float(&self) -> Option<&f64> {
        if let ControlValueSetter::Float(f) = self {
            Some(f)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_boolean(&self) -> Option<&bool> {
        if let ControlValueSetter::Boolean(f) = self {
            Some(f)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        if let ControlValueSetter::String(s) = self {
            Some(s)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let ControlValueSetter::Bytes(b) = self {
            Some(b)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_key_value(&self) -> Option<(&i128, &i128)> {
        if let ControlValueSetter::KeyValue(k, v) = self {
            Some((k, v))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_point(&self) -> Option<(&f64, &f64)> {
        if let ControlValueSetter::Point(x, y) = self {
            Some((x, y))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_enum(&self) -> Option<&i64> {
        if let ControlValueSetter::EnumValue(e) = self {
            Some(e)
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_rgb(&self) -> Option<(&f64, &f64, &f64)> {
        if let ControlValueSetter::RGB(r, g, b) = self {
            Some((r, g, b))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_menu(&self) -> Option<&u32> {
        if let ControlValueSetter::Menu(u) = self {
            Some(u)
        } else {
            None
        }
    }
}

impl Display for ControlValueSetter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlValueSetter::None => {
                write!(f, "Value: None")
            },
            ControlValueSetter::Integer(i) => {
                write!(f, "IntegerValue: {i}")
            },
            ControlValueSetter::Float(d) => {
                write!(f, "FloatValue: {d}")
            },
            ControlValueSetter::Boolean(b) => {
                write!(f, "BoolValue: {b}")
            },
            ControlValueSetter::String(s) => {
                write!(f, "StrValue: {s}")
            },
            ControlValueSetter::Bytes(b) => {
                write!(f, "BytesValue: {b:x?}")
            },
            ControlValueSetter::KeyValue(k, v) => {
                write!(f, "KVValue: ({k}, {v})")
            },
            ControlValueSetter::Point(x, y) => {
                write!(f, "PointValue: ({x}, {y})")
            },
            ControlValueSetter::EnumValue(v) => {
                write!(f, "EnumValue: {v}")
            },
            ControlValueSetter::RGB(r, g, b) => {
                write!(f, "RGBValue: ({r}, {g}, {b})")
            },
            ControlValueSetter::Menu(u) => {
                write!(f, "MenuValue: {u}")
            },
        }
    }
}

/// The list of known capture backends to the library. <br>
/// - `AUTO` is special - it tells the Camera struct to automatically choose a backend most suited for the current platform.
/// - `AVFoundation` - Uses `AVFoundation` on `MacOSX`
/// - `Video4Linux` - `Video4Linux2`, a linux specific backend.
/// - `MediaFoundation` - Microsoft Media Foundation, Windows only.
/// - `GStreamer` - Cross-platform `GStreamer` backend. Also handles IP / RTSP / HTTP / file URLs via `CameraIndex::String`.
/// - `Browser` - Uses browser APIs to capture from a webcam.
#[derive(Clone, Debug, Hash, Ord, PartialOrd, Eq, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize, Deserialize))]
pub enum ApiBackend {
    Auto,
    AVFoundation,
    Video4Linux,
    MediaFoundation,
    GStreamer,
    Browser,
    /// A custom backend not covered by the built-in variants.
    Custom(String),
}

impl Display for ApiBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Converts a MJPEG stream of `&[u8]` into a `Vec<u8>` of RGB888. (R,G,B,R,G,B,...)
/// # Errors
/// If `mozjpeg` fails to read scanlines or setup the decompressor, this will error.
/// # Safety
/// This function uses `unsafe`. The caller must ensure that:
/// - The input data is of the right size, does not exceed bounds, and/or the final size matches with the initial size.
#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[cfg_attr(feature = "docs-features", doc(cfg(feature = "mjpeg")))]
#[inline]
pub fn mjpeg_to_rgb(data: &[u8], rgba: bool) -> Result<Vec<u8>, NokhwaError> {
    use mozjpeg::Decompress;

    let mut jpeg_decompress = match Decompress::new_mem(data) {
        Ok(decompress) => {
            let decompressor_res = if rgba {
                decompress.rgba()
            } else {
                decompress.rgb()
            };
            match decompressor_res {
                Ok(decompressor) => decompressor,
                Err(why) => {
                    return Err(NokhwaError::process_frame(
                        FrameFormat::MJPEG,
                        "RGB888",
                        why.to_string(),
                    ))
                },
            }
        },
        Err(why) => {
            return Err(NokhwaError::process_frame(
                FrameFormat::MJPEG,
                "RGB888",
                why.to_string(),
            ))
        },
    };

    let scanlines_res = match jpeg_decompress.read_scanlines::<u8>() {
        Ok(v) => v,
        Err(why) => {
            return Err(NokhwaError::process_frame(
                FrameFormat::MJPEG,
                "JPEG",
                why.to_string(),
            ))
        },
    };
    // assert!(jpeg_decompress.finish_decompress());
    jpeg_decompress
        .finish()
        .map_err(|why| NokhwaError::process_frame(FrameFormat::MJPEG, "RGB888", why.to_string()))?;

    Ok(scanlines_res)
}

/// Converts MJPEG to RGB (stub for non-MJPEG/WASM builds).
/// # Errors
/// Always returns `NokhwaError::NotImplementedError` on unsupported platforms.
#[cfg(not(all(feature = "mjpeg", not(target_arch = "wasm32"))))]
pub fn mjpeg_to_rgb(_data: &[u8], _rgba: bool) -> Result<Vec<u8>, NokhwaError> {
    Err(NokhwaError::NotImplementedError(
        "Not available on WASM".to_string(),
    ))
}

/// Equivalent to [`mjpeg_to_rgb`] except with a destination buffer.
/// # Errors
/// If the decoding fails (e.g. invalid MJPEG stream), the buffer is not large enough, or you are doing this on `WebAssembly`, this will error.
#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[cfg_attr(feature = "docs-features", doc(cfg(feature = "mjpeg")))]
#[inline]
pub fn buf_mjpeg_to_rgb(data: &[u8], dest: &mut [u8], rgba: bool) -> Result<(), NokhwaError> {
    use mozjpeg::Decompress;

    let mut jpeg_decompress = match Decompress::new_mem(data) {
        Ok(decompress) => {
            let decompressor_res = if rgba {
                decompress.rgba()
            } else {
                decompress.rgb()
            };
            match decompressor_res {
                Ok(decompressor) => decompressor,
                Err(why) => {
                    return Err(NokhwaError::process_frame(
                        FrameFormat::MJPEG,
                        "RGB888",
                        why.to_string(),
                    ))
                },
            }
        },
        Err(why) => {
            return Err(NokhwaError::process_frame(
                FrameFormat::MJPEG,
                "RGB888",
                why.to_string(),
            ))
        },
    };

    // assert_eq!(dest.len(), jpeg_decompress.min_flat_buffer_size());
    if dest.len() != jpeg_decompress.min_flat_buffer_size() {
        return Err(NokhwaError::process_frame(
            FrameFormat::MJPEG,
            "RGB888",
            "Bad decoded buffer size",
        ));
    }

    jpeg_decompress
        .read_scanlines_into::<u8>(dest)
        .map_err(|why| NokhwaError::process_frame(FrameFormat::MJPEG, "RGB888", why.to_string()))?;
    // assert!(jpeg_decompress.finish_decompress());
    jpeg_decompress
        .finish()
        .map_err(|why| NokhwaError::process_frame(FrameFormat::MJPEG, "RGB888", why.to_string()))?;
    Ok(())
}

/// Converts MJPEG to RGB into a destination buffer (stub for non-MJPEG/WASM builds).
/// # Errors
/// Always returns `NokhwaError::NotImplementedError` on unsupported platforms.
#[cfg(not(all(feature = "mjpeg", not(target_arch = "wasm32"))))]
pub fn buf_mjpeg_to_rgb(_data: &[u8], _dest: &mut [u8], _rgba: bool) -> Result<(), NokhwaError> {
    Err(NokhwaError::NotImplementedError(
        "Not available on WASM".to_string(),
    ))
}

/// Returns the predicted size of the destination YUYV422 buffer.
#[must_use]
#[inline]
pub fn yuyv422_predicted_size(size: usize, rgba: bool) -> usize {
    let pixel_size = if rgba { 4 } else { 3 };
    // Each 4-byte YUYV chunk yields 2 output pixels (3 bytes each for RGB, 4 for RGBA)
    (size / 4) * (2 * pixel_size)
}

// For those maintaining this, I recommend you read: https://docs.microsoft.com/en-us/windows/win32/medfound/recommended-8-bit-yuv-formats-for-video-rendering#yuy2
// https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB
// and this too: https://stackoverflow.com/questions/16107165/convert-from-yuv-420-to-imagebgr-byte
// The YUY2(YUYV) format is a 16 bit format. We read 4 bytes at a time to get 6 bytes of RGB888.
// First, the YUY2 is converted to YCbCr 4:4:4 (4:2:2 -> 4:4:4)
// then it is converted to 6 bytes (2 pixels) of RGB888
/// Converts a YUYV 4:2:2 datastream to a RGB888 Stream. [For further reading](https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB)
/// # Errors
/// This may error when the data stream size is not divisible by 4, a i32 -> u8 conversion fails, or it fails to read from a certain index.
#[inline]
pub fn yuyv422_to_rgb(data: &[u8], rgba: bool) -> Result<Vec<u8>, NokhwaError> {
    let pixel_size = if rgba { 4 } else { 3 };
    // Each 4-byte YUYV chunk yields 2 output pixels (3 bytes each for RGB, 4 for RGBA)
    let rgb_buf_size = (data.len() / 4) * (2 * pixel_size);

    let mut dest = vec![0; rgb_buf_size];
    buf_yuyv422_to_rgb(data, &mut dest, rgba)?;

    Ok(dest)
}

/// Same as [`yuyv422_to_rgb`] but with a destination buffer instead of a return `Vec<u8>`
/// # Errors
/// If the stream is invalid YUYV, or the destination buffer is not large enough, this will error.
#[inline]
pub fn buf_yuyv422_to_rgb(data: &[u8], dest: &mut [u8], rgba: bool) -> Result<(), NokhwaError> {
    if !data.len().is_multiple_of(4) {
        return Err(NokhwaError::process_frame(
            FrameFormat::YUYV,
            "RGB888",
            "Assertion failure, the YUV stream isn't 4:2:2! (wrong number of bytes)",
        ));
    }

    let pixel_size = if rgba { 4 } else { 3 };
    // Each 4-byte YUYV chunk yields 2 output pixels (3 bytes each for RGB, 4 for RGBA)
    let rgb_buf_size = (data.len() / 4) * (2 * pixel_size);

    if dest.len() != rgb_buf_size {
        return Err(NokhwaError::process_frame(
            FrameFormat::YUYV,
            "RGB888",
            format!("Assertion failure, the destination RGB buffer is of the wrong size! [expected: {rgb_buf_size}, actual: {}]", dest.len()),
        ));
    }

    if rgba {
        crate::simd::yuyv_to_rgba_simd(data, dest);
    } else {
        crate::simd::yuyv_to_rgb_simd(data, dest);
    }

    Ok(())
}

// equation from https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB
/// Convert `YCbCr` 4:4:4 to a RGB888. [For further reading](https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB)
#[allow(clippy::many_single_char_names)]
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
#[must_use]
#[inline]
pub fn yuyv444_to_rgb(y: i32, u: i32, v: i32) -> [u8; 3] {
    let c298 = (y - 16) * 298;
    let d = u - 128;
    let e = v - 128;
    let r = ((c298 + 409 * e + 128) >> 8).clamp(0, 255) as u8;
    let g = ((c298 - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
    let b = ((c298 + 516 * d + 128) >> 8).clamp(0, 255) as u8;
    [r, g, b]
}

// equation from https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB
/// Convert `YCbCr` 4:4:4 to a RGBA8888. [For further reading](https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB)
///
/// Equivalent to [`yuyv444_to_rgb`] but with an alpha channel attached.
#[allow(clippy::many_single_char_names)]
#[must_use]
#[inline]
pub fn yuyv444_to_rgba(y: i32, u: i32, v: i32) -> [u8; 4] {
    let [r, g, b] = yuyv444_to_rgb(y, u, v);
    [r, g, b, 255]
}

/// Converts a YUYV 4:2:0 bi-planar (NV12) datastream to a RGB888 Stream. [For further reading](https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB)
/// # Errors
/// This may error when the data stream size is wrong.
#[inline]
pub fn nv12_to_rgb(
    resolution: Resolution,
    data: &[u8],
    rgba: bool,
) -> Result<Vec<u8>, NokhwaError> {
    let pxsize: usize = if rgba { 4 } else { 3 };
    let mut dest = vec![0; pxsize * resolution.width() as usize * resolution.height() as usize];
    buf_nv12_to_rgb(resolution, data, &mut dest, rgba)?;
    Ok(dest)
}

// this depresses me
// like, everytime i open this codebase all the life is sucked out of me
// i hate it
/// Converts a YUYV 4:2:0 bi-planar (NV12) datastream to a RGB888 Stream and outputs it into a destination buffer. [For further reading](https://en.wikipedia.org/wiki/YUV#Converting_between_Y%E2%80%B2UV_and_RGB)
/// # Errors
/// This may error when the data stream size is wrong.
#[allow(clippy::similar_names, clippy::cast_sign_loss)]
#[inline]
pub fn buf_nv12_to_rgb(
    resolution: Resolution,
    data: &[u8],
    out: &mut [u8],
    rgba: bool,
) -> Result<(), NokhwaError> {
    if !resolution.width().is_multiple_of(2) || !resolution.height().is_multiple_of(2) {
        return Err(NokhwaError::process_frame(
            FrameFormat::NV12,
            "RGB",
            "bad resolution",
        ));
    }

    if data.len() != (resolution.width() as usize * resolution.height() as usize * 3) / 2 {
        return Err(NokhwaError::process_frame(
            FrameFormat::NV12,
            "RGB",
            "bad input buffer size",
        ));
    }

    let pxsize: usize = if rgba { 4 } else { 3 };

    if out.len() != pxsize * resolution.width() as usize * resolution.height() as usize {
        return Err(NokhwaError::process_frame(
            FrameFormat::NV12,
            "RGB",
            "bad output buffer size",
        ));
    }

    crate::simd::nv12_to_rgb_simd(
        resolution.width() as usize,
        resolution.height() as usize,
        data,
        out,
        rgba,
    );

    Ok(())
}

/// Extracts the Y (luma) channel from a YUYV 4:2:2 stream.
///
/// YUYV stores pairs of pixels as [Y0, U, Y1, V]. This function extracts
/// every Y byte without any color-space conversion, producing one luma byte
/// per pixel.
///
/// # Errors
/// Returns an error if the input size is not divisible by 4, or the
/// destination buffer is the wrong size.
#[inline]
pub fn buf_yuyv_extract_luma(data: &[u8], dest: &mut [u8]) -> Result<(), NokhwaError> {
    if !data.len().is_multiple_of(4) {
        return Err(NokhwaError::process_frame(
            FrameFormat::YUYV,
            "Luma",
            "YUYV stream length not divisible by 4",
        ));
    }

    let pixel_count = data.len() / 2;
    if dest.len() != pixel_count {
        return Err(NokhwaError::process_frame(
            FrameFormat::YUYV,
            "Luma",
            format!(
                "destination buffer size mismatch (expected {pixel_count}, got {})",
                dest.len()
            ),
        ));
    }

    crate::simd::yuyv_extract_luma_simd(data, dest);

    Ok(())
}

/// Extracts the Y (luma) plane from an NV12 (YUV 4:2:0 bi-planar) stream.
///
/// NV12 stores a full-resolution Y plane followed by a half-resolution
/// interleaved UV plane. This function copies only the Y plane, producing
/// one luma byte per pixel with zero color-space conversion.
///
/// # Errors
/// Returns an error if the input or destination buffer sizes are incorrect.
#[inline]
pub fn buf_nv12_extract_luma(
    resolution: Resolution,
    data: &[u8],
    dest: &mut [u8],
) -> Result<(), NokhwaError> {
    if !resolution.width().is_multiple_of(2) || !resolution.height().is_multiple_of(2) {
        return Err(NokhwaError::process_frame(
            FrameFormat::NV12,
            "Luma",
            "bad resolution",
        ));
    }

    let w = resolution.width() as usize;
    let h = resolution.height() as usize;
    let y_size = w * h;
    let expected_input = y_size * 3 / 2;

    if data.len() != expected_input {
        return Err(NokhwaError::process_frame(
            FrameFormat::NV12,
            "Luma",
            format!(
                "NV12 input size mismatch (expected {expected_input}, got {})",
                data.len()
            ),
        ));
    }

    if dest.len() != y_size {
        return Err(NokhwaError::process_frame(
            FrameFormat::NV12,
            "Luma",
            format!(
                "destination buffer size mismatch (expected {y_size}, got {})",
                dest.len()
            ),
        ));
    }

    dest.copy_from_slice(&data[..y_size]);
    Ok(())
}

/// Converts a BGR datastream to RGB, writing into the provided output buffer.
/// # Errors
/// Returns `NokhwaError::ProcessFrameError` if the resolution or data size is invalid.
#[allow(clippy::similar_names)]
#[inline]
pub fn buf_bgr_to_rgb(
    resolution: Resolution,
    data: &[u8],
    out: &mut [u8],
) -> Result<(), NokhwaError> {
    let width = resolution.width();
    let height = resolution.height();

    let input_size = width as usize * height as usize * 3; // BGR is 3 bytes per pixel
    let output_size = width as usize * height as usize * 3; // RGB is 3 bytes per pixel

    if data.len() != input_size {
        return Err(NokhwaError::process_frame(
            FrameFormat::RAWBGR,
            "RGB",
            "bad input buffer size",
        ));
    }

    if out.len() != output_size {
        return Err(NokhwaError::process_frame(
            FrameFormat::RAWBGR,
            "RGB",
            "bad output buffer size",
        ));
    }

    crate::simd::bgr_to_rgb_simd(data, out);

    Ok(())
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
