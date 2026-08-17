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

#![cfg(feature = "wgpu-types")]

use crate::{
    error::NokhwaError,
    types::{FrameFormat, Resolution},
};
use wgpu::{Extent3d, Texture as WgpuTexture, TextureFormat};

/// A GPU texture containing raw camera frame data in its native pixel format,
/// without conversion to RGBA. Consumers can use GPU shaders to decode the
/// data according to the [`FrameFormat`] and [`TextureFormat`] provided.
#[derive(Debug)]
pub struct RawTextureData {
    /// The wgpu texture containing the raw frame data.
    pub texture: WgpuTexture,
    /// The camera's native pixel format (e.g. NV12, YUYV).
    pub source_frame_format: FrameFormat,
    /// The wgpu texture format used to store the data.
    pub texture_format: TextureFormat,
    /// The original frame resolution.
    pub resolution: Resolution,
}

/// Returns the wgpu [`TextureFormat`] and texture dimensions suitable for
/// storing raw frame data of the given [`FrameFormat`] and [`Resolution`].
///
/// For formats without a direct wgpu equivalent, the data is packed into a
/// single-channel or two-channel texture that a shader can decode.
///
/// # Errors
/// Returns `Err` for [`FrameFormat::MJPEG`] because compressed MJPEG data
/// cannot be uploaded as a raw texture — use
/// [`FrameSource::frame_texture()`](crate::traits::FrameSource::frame_texture)
/// instead, which decodes to RGBA first.
pub fn raw_texture_layout(
    format: FrameFormat,
    resolution: Resolution,
) -> Result<(TextureFormat, Extent3d, u32), NokhwaError> {
    let w = resolution.width();
    let h = resolution.height();
    match format {
        // YUYV: 2 bytes per pixel, packed as [Y0 U0 Y1 V0]. Store as Rg8Unorm
        // so each texel holds one byte-pair. Width is the full pixel width
        // (each texel = 1 pixel channel), but total bytes per row = 2*w.
        FrameFormat::YUYV => {
            if !w.is_multiple_of(2) {
                return Err(NokhwaError::process_frame(
                    format,
                    "RawTextureData",
                    format!("YUYV requires even width, got {w}"),
                ));
            }
            Ok((
                TextureFormat::Rg8Unorm,
                Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                2 * w,
            ))
        },
        // NV12: Y plane (w*h bytes) + interleaved UV plane (w*h/2 bytes).
        // Total = 1.5 * w * h bytes. Store as R8Unorm with height * 3/2.
        FrameFormat::NV12 => {
            if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
                return Err(NokhwaError::process_frame(
                    format,
                    "RawTextureData",
                    format!("NV12 requires even dimensions, got {w}x{h}"),
                ));
            }
            Ok((
                TextureFormat::R8Unorm,
                Extent3d {
                    width: w,
                    height: h * 3 / 2,
                    depth_or_array_layers: 1,
                },
                w,
            ))
        },
        // GRAY: 1 byte per pixel. Directly maps to R8Unorm.
        FrameFormat::GRAY => Ok((
            TextureFormat::R8Unorm,
            Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            w,
        )),
        // RAWRGB / RAWBGR: 3 bytes per pixel. No exact 3-channel unorm in
        // wgpu, so use R8Unorm with width*3 to avoid any padding requirement.
        FrameFormat::RAWRGB | FrameFormat::RAWBGR => Ok((
            TextureFormat::R8Unorm,
            Extent3d {
                width: w * 3,
                height: h,
                depth_or_array_layers: 1,
            },
            w * 3,
        )),
        // MJPEG is a compressed format — raw bytes cannot be uploaded directly.
        FrameFormat::MJPEG => Err(NokhwaError::general(
            "frame_texture_raw() cannot be used with MJPEG sources; \
             use frame_texture() instead",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuyv_layout_uses_rg8unorm_with_double_stride() {
        let (fmt, ext, stride) =
            raw_texture_layout(FrameFormat::YUYV, Resolution::new(640, 480)).unwrap();
        assert_eq!(fmt, TextureFormat::Rg8Unorm);
        assert_eq!(ext.width, 640);
        assert_eq!(ext.height, 480);
        assert_eq!(ext.depth_or_array_layers, 1);
        assert_eq!(stride, 1280);
    }

    #[test]
    fn yuyv_rejects_odd_width() {
        // The `error` field carries the offending width so callers can
        // log / surface what got rejected. The previous test
        // destructured `..` and ignored the message; a regression that
        // dropped the actual width from the diagnostic
        // (e.g. "YUYV requires even width") would still satisfy the
        // variant + src + destination check while making the error
        // useless for triage. Pin the exact format string including
        // the bad value.
        let err = raw_texture_layout(FrameFormat::YUYV, Resolution::new(641, 480)).unwrap_err();
        match err {
            NokhwaError::ProcessFrameError {
                src,
                destination,
                error,
            } => {
                assert_eq!(src, FrameFormat::YUYV);
                assert_eq!(destination, "RawTextureData");
                assert_eq!(error, "YUYV requires even width, got 641");
            },
            other => panic!("expected ProcessFrameError, got {other:?}"),
        }
    }

    #[test]
    fn nv12_layout_uses_r8unorm_with_3_2_height() {
        let (fmt, ext, stride) =
            raw_texture_layout(FrameFormat::NV12, Resolution::new(1920, 1080)).unwrap();
        assert_eq!(fmt, TextureFormat::R8Unorm);
        assert_eq!(ext.width, 1920);
        assert_eq!(ext.height, 1620);
        assert_eq!(stride, 1920);
    }

    #[test]
    fn nv12_rejects_odd_dimensions() {
        // Previously `is_err()`-only across three resolutions; a
        // regression that returned a different error variant
        // (e.g. `NokhwaError::general` instead of
        // `ProcessFrameError`) — making the NV12 path inconsistent
        // with the YUYV path that callers may switch on by variant —
        // would have passed. Pin the variant, src, destination, and
        // exact error string for every odd-dim case so each width /
        // height combination is verified to surface the offending
        // dimensions verbatim in the message.
        for (w, h) in [(641u32, 480u32), (640, 481), (641, 481)] {
            let err = raw_texture_layout(FrameFormat::NV12, Resolution::new(w, h)).unwrap_err();
            match err {
                NokhwaError::ProcessFrameError {
                    src,
                    destination,
                    error,
                } => {
                    assert_eq!(src, FrameFormat::NV12, "{w}x{h}");
                    assert_eq!(destination, "RawTextureData", "{w}x{h}");
                    assert_eq!(error, format!("NV12 requires even dimensions, got {w}x{h}"));
                },
                other => panic!("expected ProcessFrameError for {w}x{h}, got {other:?}"),
            }
        }
    }

    #[test]
    fn gray_layout_is_r8unorm_one_byte_per_pixel() {
        let (fmt, ext, stride) =
            raw_texture_layout(FrameFormat::GRAY, Resolution::new(320, 240)).unwrap();
        assert_eq!(fmt, TextureFormat::R8Unorm);
        assert_eq!(ext.width, 320);
        assert_eq!(ext.height, 240);
        assert_eq!(stride, 320);
    }

    #[test]
    fn raw_rgb_layout_is_r8unorm_with_3x_width() {
        let (fmt, ext, stride) =
            raw_texture_layout(FrameFormat::RAWRGB, Resolution::new(640, 480)).unwrap();
        assert_eq!(fmt, TextureFormat::R8Unorm);
        assert_eq!(ext.width, 1920);
        assert_eq!(ext.height, 480);
        assert_eq!(stride, 1920);
    }

    #[test]
    fn raw_bgr_layout_matches_rawrgb_shape() {
        let rgb = raw_texture_layout(FrameFormat::RAWRGB, Resolution::new(640, 480)).unwrap();
        let bgr = raw_texture_layout(FrameFormat::RAWBGR, Resolution::new(640, 480)).unwrap();
        assert_eq!(rgb, bgr);
    }

    #[test]
    fn mjpeg_is_rejected_with_general_error() {
        // Hardened from two contains-only checks. The diagnostic
        // points users from `frame_texture_raw()` to `frame_texture()`
        // — a fixed user-facing message. A regression that subtly
        // changed the wording (e.g. dropping the parens after
        // `frame_texture` or changing "cannot be used" to "is not
        // supported") would slip past `.contains("MJPEG")` /
        // `.contains("frame_texture()")` while breaking any
        // documentation or downstream test that quotes the exact
        // string. Pin the full message verbatim, matching the source
        // at `wgpu.rs:121`.
        let err = raw_texture_layout(FrameFormat::MJPEG, Resolution::new(640, 480)).unwrap_err();
        match err {
            NokhwaError::GeneralError { message, backend } => {
                assert_eq!(
                    message,
                    "frame_texture_raw() cannot be used with MJPEG sources; \
                     use frame_texture() instead"
                );
                // The branch at `wgpu.rs:121` calls `NokhwaError::general(...)`
                // which always sets `backend: None`. Pin so a future refactor
                // that re-routes through a backend-tagged constructor (e.g.
                // `NokhwaError::GeneralError { backend: Some(...), .. }`)
                // changes the public-API shape and is caught.
                assert!(
                    backend.is_none(),
                    "expected no backend tag, got {backend:?}"
                );
            },
            other => panic!("expected GeneralError, got {other:?}"),
        }
    }
}
