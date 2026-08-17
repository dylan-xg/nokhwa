use crate::buffer::{Buffer, TimestampKind};
use crate::error::NokhwaError;
#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
use crate::format_types::Mjpeg;
use crate::format_types::{Gray, Nv12, RawBgr, RawRgb, Yuyv};
use crate::frame::{
    convert_to_luma, convert_to_rgb, convert_to_rgb_buffer, convert_to_rgba,
    convert_to_rgba_buffer, Frame, IntoLuma, IntoRgb, IntoRgba,
};
use crate::types::{FrameFormat, Resolution};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Frame construction
// ---------------------------------------------------------------------------

#[test]
fn frame_new_rawrgb() {
    let data = vec![255u8; 2 * 2 * 3]; // 2x2 RGB
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    assert_eq!(frame.resolution(), Resolution::new(2, 2));
    assert_eq!(frame.buffer().len(), 12);
}

#[test]
fn frame_new_gray() {
    let data = vec![128u8; 4]; // 2x2 gray
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let frame: Frame<Gray> = Frame::new(buf);
    assert_eq!(frame.resolution(), Resolution::new(2, 2));
}

#[test]
fn frame_into_buffer_roundtrip() {
    let data = vec![100u8; 12];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let recovered = frame.into_buffer();
    assert_eq!(recovered.buffer(), &data[..]);
    // `Frame::into_buffer` is documented as "consumes the frame and
    // returns the underlying [`Buffer`]" (`nokhwa-core/src/frame.rs:112-116`).
    // The bytes-only check above would still pass after a refactor
    // that cloned the buffer bytes into a freshly-constructed
    // `Buffer::new(Resolution::default(), ...)` — silently zeroing
    // the resolution and dropping the original `FrameFormat`. Pin
    // both side-channel fields so the round-trip is whole-struct
    // identity, not just data-identity.
    assert_eq!(recovered.resolution(), Resolution::new(2, 2));
    assert_eq!(recovered.source_frame_format(), FrameFormat::RAWRGB);
}

#[test]
fn frame_try_new_matching_succeeds() {
    let data = vec![128u8; 4];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let result = Frame::<Gray>::try_new(buf);
    assert!(result.is_ok());
}

/// Pin the *shape* of the error returned by `Frame::try_new` on
/// format mismatch. `nokhwa-core/src/frame.rs:76-92` returns
/// `NokhwaError::ProcessFrameError { src, destination, error }`
/// with `src = buffer.source_frame_format()` and
/// `destination = format!("Frame<{:?}>", F::FRAME_FORMAT)`.
///
/// The existing `frame_try_new_mismatch_returns_error` only asserts
/// `result.is_err()`. A regression that swapped to a different
/// variant (e.g. `NokhwaError::OpenDeviceError`) or scrambled the
/// `src`/`destination` fields would slip past that check — but
/// callers `match`ing on `NokhwaError::ProcessFrameError { src, .. }`
/// to render diagnostics would silently break.
#[test]
fn frame_try_new_mismatch_error_carries_src_and_destination() {
    let data = vec![128u8; 4]; // 2x2 gray
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let err = Frame::<RawRgb>::try_new(buf).expect_err("GRAY buffer cannot become Frame<RawRgb>");
    match err {
        NokhwaError::ProcessFrameError {
            src,
            destination,
            error,
        } => {
            assert_eq!(
                src,
                FrameFormat::GRAY,
                "src must be the buffer's actual FrameFormat"
            );
            // Pin the exact destination string. A weaker `contains`
            // would stay green even if the wrapping changed from
            // `Frame<RAWRGB>` to e.g. `RAWRGB` or `Frame:RAWRGB`,
            // breaking downstream log/grep tooling.
            assert_eq!(destination, "Frame<RAWRGB>");
            // Pin the exact diagnostic string. A weaker `contains`
            // pair would stay green if the order was swapped to
            // `expected GRAY, got RAWRGB`, which would mislead
            // anyone reading the error.
            assert_eq!(error, "expected RAWRGB, got GRAY");
        },
        other => panic!("expected NokhwaError::ProcessFrameError, got {other:?}"),
    }
}

#[test]
#[should_panic(expected = "Buffer FrameFormat")]
fn frame_new_panics_on_format_mismatch() {
    // `Frame::new` is the infallible variant — it must `assert_eq!`
    // that the buffer's `FrameFormat` matches `F::FRAME_FORMAT`. If
    // the assert is silently weakened to e.g. a `debug_assert!`,
    // release builds would silently produce a `Frame` with
    // type-tag/data disagreement and decode to garbage. Pin the
    // panic so any future regression that drops the runtime check
    // is caught in CI.
    let data = vec![0u8; 4];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let _frame: Frame<RawRgb> = Frame::new(buf);
}

#[test]
fn frame_accessors_delegate_to_buffer() {
    // Pin `Frame::resolution()`, `buffer()`, and `as_buffer()` as
    // delegating to the underlying `Buffer` — a refactor that
    // forgets to forward, or that reads from a stale field on
    // `Frame`, would silently desync the typed handle from its
    // payload.
    let data: Vec<u8> = (0..12u8).collect();
    let res = Resolution::new(2, 2);
    let buf = Buffer::new(res, &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);

    assert_eq!(frame.resolution(), res);
    assert_eq!(frame.buffer(), &data[..]);
    assert_eq!(frame.as_buffer().resolution(), res);
    assert_eq!(frame.as_buffer().buffer(), &data[..]);
    assert_eq!(frame.as_buffer().source_frame_format(), FrameFormat::RAWRGB);
}

#[test]
fn frame_capture_timestamp_passthrough_some() {
    // `Frame::capture_timestamp{,_with_kind}` must forward the
    // backend-provided timestamp from the underlying `Buffer`. A
    // refactor that drops the kind, returns `None`, or rebuilds
    // the `Duration` with a different reference clock would
    // silently mis-stamp every frame.
    let data = vec![0u8; 4];
    let ts = Duration::from_millis(12_345);
    let buf = Buffer::with_timestamp(
        Resolution::new(2, 2),
        &data,
        FrameFormat::GRAY,
        Some((ts, TimestampKind::MonotonicClock)),
    );
    let frame: Frame<Gray> = Frame::new(buf);

    assert_eq!(frame.capture_timestamp(), Some(ts));
    assert_eq!(
        frame.capture_timestamp_with_kind(),
        Some((ts, TimestampKind::MonotonicClock))
    );
}

#[test]
fn frame_capture_timestamp_passthrough_none() {
    // The `None` case must also be forwarded — a regression that
    // synthesises a fake "now" timestamp when the backend didn't
    // provide one would let downstream code believe every frame
    // is timestamped, which is the whole reason the field is an
    // `Option`.
    let data = vec![0u8; 4];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let frame: Frame<Gray> = Frame::new(buf);

    assert_eq!(frame.capture_timestamp(), None);
    assert_eq!(frame.capture_timestamp_with_kind(), None);
}

// ---------------------------------------------------------------------------
// RGB conversion
// ---------------------------------------------------------------------------

#[test]
fn rawrgb_into_rgb_materialize() {
    let data = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    assert_eq!(img.width(), 2);
    assert_eq!(img.height(), 2);
    // Raw RGB passthrough: first pixel should match
    assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30]);
}

#[test]
fn rawbgr_into_rgb_swaps_channels() {
    // BGR: B=10, G=20, R=30
    let data = vec![10, 20, 30, 10, 20, 30, 10, 20, 30, 10, 20, 30];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    // Should be swapped to R=30, G=20, B=10
    assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10]);
}

#[test]
fn rawrgb_into_rgb_write_to() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 12];
    frame.into_rgb().write_to(&mut dest).unwrap();
    assert_eq!(dest, data);
}

// `Frame<RawBgr>::into_rgb().write_to(...)` routes through
// `convert_to_rgb_buffer`'s `FrameFormat::RAWBGR => buf_bgr_to_rgb(...)`
// arm at frame.rs:431. The materialize counterpart is pinned by
// `rawbgr_into_rgb_swaps_channels` above, but the `write_to` end-to-end
// integration was uncovered: a regression in the dispatcher wiring
// (e.g. accidentally routing RAWBGR through the RAWRGB no-op copy)
// would silently produce blue-tinted "RGB" output and pass the
// materialize test only by virtue of going through `data.to_vec()`
// — and `write_to` callers would not catch it.

#[test]
fn rawbgr_into_rgb_write_to_swaps_channels() {
    // Per-pixel-distinct B/G/R values so the test fails loudly if the
    // dispatcher ever stops swapping (e.g. accidentally lands in the
    // RAWRGB no-op arm).
    // BGR: B=10, G=20, R=30  →  RGB: R=30, G=20, B=10
    // BGR: B=40, G=80, R=120 →  RGB: R=120, G=80, B=40
    // Repeated to fill 2×2 (buf_bgr_to_rgb requires multiple-of-2 dims).
    let data = vec![
        10, 20, 30, 40, 80, 120, // row 0
        10, 20, 30, 40, 80, 120, // row 1
    ];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let mut dest = vec![0u8; 12];
    frame.into_rgb().write_to(&mut dest).unwrap();
    assert_eq!(
        dest,
        vec![30, 20, 10, 120, 80, 40, 30, 20, 10, 120, 80, 40,],
        "write_to must produce per-pixel B↔R-swapped RGB",
    );
}

#[test]
fn rawbgr_into_rgb_write_to_rejects_mismatched_dest() {
    // 2×2 RAWBGR needs 12 bytes in and 12 out. Pass an 11-byte dest
    // to hit the `out.len() != output_size` guard inside
    // `buf_bgr_to_rgb` at types.rs:1951. The error must report
    // FrameFormat::RAWBGR as the source — proving the dispatch path
    // hands off to `buf_bgr_to_rgb` (which uses the RAWBGR src
    // hardcoded inside) rather than wrapping the call with its own
    // synthesized error.
    let data = vec![10, 20, 30, 40, 80, 120, 10, 20, 30, 40, 80, 120];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let mut dest = vec![0u8; 11]; // expected 12
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::RAWBGR, "RGB", "bad output buffer size");
}

#[test]
fn rawrgb_into_rgb_write_png_emits_valid_png() {
    // `RgbConversion::write_png` is a public API that pipes through
    // `image::DynamicImage::write_to(_, ImageFormat::Png)`. It had
    // zero coverage: a regression that drops the PNG codec from the
    // `image` dependency or that flips the `ImageFormat` argument
    // would silently produce empty / wrong-format output, or worse,
    // an `Err` that callers learn about only at runtime. Pin the
    // happy path by writing into an in-memory `Cursor`, then assert
    // (a) the call succeeds, (b) the output starts with the PNG
    // magic bytes (`\x89PNG\r\n\x1a\n`), (c) the output is large
    // enough to plausibly contain a 2×2 image (PNG signature alone
    // is 8 bytes; a real image with IHDR/IDAT/IEND is ≥ ~50 bytes).
    let data: Vec<u8> = (0..12u8).collect();
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);

    let mut sink: std::io::Cursor<Vec<u8>> = std::io::Cursor::new(Vec::new());
    frame
        .into_rgb()
        .write_png(&mut sink)
        .expect("write_png must succeed for a valid 2x2 RAWRGB frame");

    let bytes = sink.into_inner();
    let png_magic = [0x89u8, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    assert!(
        bytes.starts_with(&png_magic),
        "output must begin with PNG magic, got first 8 bytes: {:?}",
        bytes.iter().take(8).collect::<Vec<_>>()
    );
    assert!(
        bytes.len() >= 50,
        "PNG output too small to be a real image (got {} bytes)",
        bytes.len()
    );
}

// ---------------------------------------------------------------------------
// RGBA conversion
// ---------------------------------------------------------------------------

#[test]
fn rawrgb_into_rgba_adds_alpha() {
    let data = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let img = frame.into_rgba().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30, 255]);
}

#[test]
fn rawbgr_into_rgba_swaps_and_adds_alpha() {
    let data = vec![10, 20, 30, 10, 20, 30, 10, 20, 30, 10, 20, 30];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let img = frame.into_rgba().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [30, 20, 10, 255]);
}

// `RgbaConversion::write_to` for RAWRGB and RAWBGR routes through
// `convert_to_rgba_buffer`, distinct from `convert_to_rgba` taken by
// `materialize()`. Mismatched-dest guards and the
// `not_multiple_of_3` guard are pinned for both formats just below;
// these are the matching happy-path pins. A regression that drops
// the alpha byte, swaps RGBA channel order, or — for RAWBGR —
// forgets to swap the B/R channels in the buffer branch would slip
// past the materialize-only suite and produce blue-tinted "RGBA" or
// transparent output for streaming consumers that pre-allocate.

#[test]
fn rawrgb_into_rgba_write_to_appends_alpha() {
    let data = vec![10u8, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 4];
    frame
        .into_rgba()
        .write_to(&mut dest)
        .expect("RAWRGB write_to RGBA");
    // Pixel 0: [10, 20, 30, 255]
    assert_eq!(&dest[0..4], &[10, 20, 30, 255]);
    // Pixel 3: [100, 110, 120, 255]
    assert_eq!(&dest[12..16], &[100, 110, 120, 255]);
}

#[test]
fn rawbgr_into_rgba_write_to_swaps_and_appends_alpha() {
    // Per-pixel-distinct B/G/R values so a missing swap surfaces
    // immediately as the wrong channel order.
    // BGR (10, 20, 30) → RGBA (30, 20, 10, 255)
    // BGR (40, 80, 120) → RGBA (120, 80, 40, 255)
    let data = vec![10u8, 20, 30, 40, 80, 120, 10, 20, 30, 40, 80, 120];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 4];
    frame
        .into_rgba()
        .write_to(&mut dest)
        .expect("RAWBGR write_to RGBA");
    assert_eq!(
        &dest[0..4],
        &[30, 20, 10, 255],
        "BGR (10,20,30) must round-trip to RGBA (30,20,10,255)"
    );
    assert_eq!(
        &dest[4..8],
        &[120, 80, 40, 255],
        "BGR (40,80,120) must round-trip to RGBA (120,80,40,255)"
    );
}

// `Frame<Nv12>::into_rgb` / `into_rgba` runs YCbCr 4:2:0 → RGB
// through `nv12_to_rgb_simd` (BT.601 video-range). The error
// guards on `buf_nv12_to_rgb` are pinned in `types_tests.rs`, but
// the actual color-decode kernel — coefficient table, channel
// order, alpha placement — had no end-to-end pixel-output check.
// A regression that swaps R / B (`yuyv444_to_rgb` channel order),
// uses studio-range coefficients on full-range Y, or zeroes the
// alpha byte would silently corrupt every NV12 frame.

#[test]
fn nv12_into_rgb_video_range_black_decodes_to_zero() {
    // Video-range black is Y=16, U=V=128 (no chroma offset).
    // BT.601: R = ((16-16)*298 + 0) >> 8 = 0. Pin so a future
    // tweak to the kernel's pre-offset constant 16 (the mythical
    // "let's accept full-range input") would shift black off
    // zero.
    let mut data = vec![16u8; 4]; // 2x2 Y plane
    data.extend_from_slice(&[128, 128]); // 1 UV pair
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert!(
                p[0] <= 1 && p[1] <= 1 && p[2] <= 1,
                "video-range black must decode to ~(0,0,0), got {p:?} at ({x},{y})"
            );
        }
    }
}

#[test]
fn nv12_into_rgb_video_range_white_clamps_to_max() {
    // Y=255 with neutral chroma → ((255-16)*298) >> 8 = 278;
    // saturating_clamp(0..=255) yields 255. Pin the clamp so a
    // regression that uses raw `as u8` truncation (278 % 256 =
    // 22) would surface as wraparound to dim pixels.
    let mut data = vec![255u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert_eq!(
                p,
                [255, 255, 255],
                "Y=255 with neutral chroma must clamp to (255,255,255), got {p:?} at ({x},{y})"
            );
        }
    }
}

#[test]
fn nv12_into_rgb_neutral_chroma_produces_gray() {
    // Neutral chroma (U=V=128) means R, G, B share the same Y-
    // derived value — the output is a true grayscale, identical
    // across the three channels. Pin so a regression that
    // accidentally feeds the Cr term into the G coefficient (or
    // any other channel transposition) is caught.
    let mut data = vec![100u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    let p = img.get_pixel(0, 0).0;
    assert_eq!(
        p[0], p[1],
        "neutral-chroma R should equal G, got R={} G={}",
        p[0], p[1]
    );
    assert_eq!(
        p[1], p[2],
        "neutral-chroma G should equal B, got G={} B={}",
        p[1], p[2]
    );
}

#[test]
fn nv12_into_rgba_appends_opaque_alpha() {
    // The NV12→RGBA path shares the YCbCr decode with NV12→RGB
    // and overlays alpha=255 at every 4th byte. Pin so a tweak
    // that copies Y into the alpha slot (or zeroes alpha) is
    // caught.
    let mut data = vec![128u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let img = frame.into_rgba().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert_eq!(
                p[3], 255,
                "NV12→RGBA alpha must be 255, got {} at ({x},{y})",
                p[3]
            );
        }
    }
}

// `Frame<Yuyv>::into_rgb` / `into_rgba` runs YCbCr 4:2:2 → RGB
// through `yuyv_to_rgb_simd` (BT.601, same coefficient table as
// NV12). The error guards on `buf_yuyv422_to_rgb` and the kernel
// math for NV12 are pinned, but YUYV → RGB had no end-to-end
// pixel-output assertion. Symmetric risk to the NV12 path: a
// regression that swaps R / B, mishandles the interleaved
// `[Y0,U,Y1,V]` layout (e.g. reads V as U), or zeroes the alpha
// byte would silently corrupt every YUYV frame from popular
// USB UVC webcams.

#[test]
fn yuyv_into_rgb_video_range_black_decodes_to_zero() {
    // 2x2 YUYV: 2 chunks of `[Y0, U, Y1, V]`. Y=16 + U=V=128 is
    // BT.601 video-range black: R = ((16-16)*298) >> 8 = 0. Pin
    // the (Y-16) pre-offset so a future "let's accept full-range
    // input" tweak shifts black off zero.
    let data = vec![16, 128, 16, 128, 16, 128, 16, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert!(
                p[0] <= 1 && p[1] <= 1 && p[2] <= 1,
                "video-range black must decode to ~(0,0,0), got {p:?} at ({x},{y})"
            );
        }
    }
}

#[test]
fn yuyv_into_rgb_video_range_white_clamps_to_max() {
    // Y=255 on every Y0/Y1 with neutral chroma. Decode yields
    // 278 → clamp(0..=255) = 255. Pin so a regression that uses
    // raw `as u8` truncation surfaces as wraparound to dim
    // pixels (278 % 256 = 22) instead of the expected white.
    let data = vec![255, 128, 255, 128, 255, 128, 255, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert_eq!(
                p,
                [255, 255, 255],
                "Y=255 with neutral chroma must clamp to (255,255,255), got {p:?} at ({x},{y})"
            );
        }
    }
}

#[test]
fn yuyv_into_rgb_neutral_chroma_produces_gray() {
    // Neutral chroma → R=G=B. Catches a regression that feeds
    // U into the V coefficient or vice versa (the YUYV chunk
    // layout `[Y0, U, Y1, V]` is easy to mis-index — V at
    // offset 3 is *one* read away from accidentally being read
    // as U).
    let data = vec![100, 128, 100, 128, 100, 128, 100, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert_eq!(
                p[0], p[1],
                "neutral-chroma R should equal G at ({x},{y}), got R={} G={}",
                p[0], p[1]
            );
            assert_eq!(
                p[1], p[2],
                "neutral-chroma G should equal B at ({x},{y}), got G={} B={}",
                p[1], p[2]
            );
        }
    }
}

#[test]
fn yuyv_into_rgba_appends_opaque_alpha() {
    // The YUYV→RGBA path shares the YCbCr decode with YUYV→RGB
    // and writes alpha=255 at every 4th byte. Pin so a tweak
    // that copies Y into the alpha slot or zeroes alpha is
    // caught.
    let data = vec![128, 128, 128, 128, 128, 128, 128, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let img = frame.into_rgba().materialize().unwrap();
    for y in 0..2 {
        for x in 0..2 {
            let p = img.get_pixel(x, y).0;
            assert_eq!(
                p[3], 255,
                "YUYV→RGBA alpha must be 255, got {} at ({x},{y})",
                p[3]
            );
        }
    }
}

// `RgbConversion::write_to` and `RgbaConversion::write_to` are the
// zero-copy production path — streaming pipelines that pre-allocate
// once and reuse the buffer take this branch instead of `materialize`.
// The `materialize` path for NV12 / YUYV is pinned just above; the
// `write_to` path goes through `convert_to_rgb_buffer` /
// `convert_to_rgba_buffer`, separate functions with their own
// dest-size guard. Without these tests, a regression in the buffer
// branch (wrong stride, wrong dest length, or quietly falling back
// to a no-op) would slip past the materialize-only suite and silently
// corrupt every NV12 / YUYV streaming consumer.

#[test]
fn nv12_into_rgb_write_to_neutral_chroma_produces_gray() {
    let mut data = vec![100u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 3];
    frame
        .into_rgb()
        .write_to(&mut dest)
        .expect("NV12 write_to RGB");
    for px in dest.as_chunks::<3>().0 {
        assert_eq!(
            px[0], px[1],
            "NV12 write_to RGB neutral-chroma R must equal G, got R={} G={}",
            px[0], px[1]
        );
        assert_eq!(
            px[1], px[2],
            "NV12 write_to RGB neutral-chroma G must equal B, got G={} B={}",
            px[1], px[2]
        );
    }
}

#[test]
fn nv12_into_rgba_write_to_appends_opaque_alpha() {
    let mut data = vec![128u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 4];
    frame
        .into_rgba()
        .write_to(&mut dest)
        .expect("NV12 write_to RGBA");
    for px in dest.as_chunks::<4>().0 {
        assert_eq!(
            px[3], 255,
            "NV12 write_to RGBA alpha must be 255, got {}",
            px[3]
        );
    }
}

// `buf_nv12_to_rgb` (`nokhwa-core/src/types.rs:1795-1834`) has three
// distinct guards before dispatching into `crate::simd::nv12_to_rgb_simd`:
//   1. resolution divisibility — both width and height must be even
//      (NV12's UV plane is half-resolution in both dimensions)
//   2. `data.len() == w * h * 3 / 2` — input plane sizes
//   3. `out.len() == pxsize * w * h` — output buffer size
//
// Symmetric to the YUYV rejection tests added in #314. Without these
// pins, a regression that dropped any guard (e.g. a refactor that
// folded NV12 + YUYV into a shared helper and lost the resolution
// parity check) would surface as a SIMD out-of-bounds panic or
// silent garbage output rather than a clean `ProcessFrameError`.

#[test]
fn nv12_into_rgb_write_to_rejects_odd_resolution() {
    // 3×3 fails the resolution-parity guard at types.rs:1801. We need
    // a buffer with `w*h*3/2 = 13` bytes (rounded down from 13.5),
    // but the parity check fires first regardless of input size.
    // Use a 3×2 frame (parity fails on width) with arbitrary data.
    let data = vec![100u8; 3 * 2 * 3 / 2]; // 9 bytes
    let buf = Buffer::new(Resolution::new(3, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 3 * 2 * 3];
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::NV12, "RGB", "bad resolution");
}

#[test]
fn nv12_into_rgb_write_to_rejects_wrong_input_size() {
    // 2×2 needs 6 bytes (4 Y + 2 UV). Pass 5 to trip the input-size
    // guard at types.rs:1809 after parity check passes.
    let data = vec![100u8, 100, 100, 100, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 3];
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::NV12, "RGB", "bad input buffer size");
}

#[test]
fn nv12_into_rgb_write_to_rejects_mismatched_dest() {
    // 2×2 valid input (6 bytes). Expected RGB dest = 12 bytes; pass
    // 9 to trip the output-size guard at types.rs:1819.
    let mut data = vec![100u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 9];
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::NV12, "RGB", "bad output buffer size");
}

#[test]
fn nv12_into_rgba_write_to_rejects_odd_resolution() {
    let data = vec![100u8; 3 * 2 * 3 / 2];
    let buf = Buffer::new(Resolution::new(3, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 3 * 2 * 4];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::NV12, "RGB", "bad resolution");
}

#[test]
fn nv12_into_rgba_write_to_rejects_wrong_input_size() {
    let data = vec![100u8, 100, 100, 100, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 4];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::NV12, "RGB", "bad input buffer size");
}

#[test]
fn nv12_into_rgba_write_to_rejects_mismatched_dest() {
    // 2×2 valid input. Expected RGBA dest = 16 bytes; pass 12.
    let mut data = vec![100u8; 4];
    data.extend_from_slice(&[128, 128]);
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 12];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::NV12, "RGB", "bad output buffer size");
}

#[test]
fn yuyv_into_rgb_write_to_neutral_chroma_produces_gray() {
    let data = vec![100u8, 128, 100, 128, 100, 128, 100, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 3];
    frame
        .into_rgb()
        .write_to(&mut dest)
        .expect("YUYV write_to RGB");
    for px in dest.as_chunks::<3>().0 {
        assert_eq!(
            px[0], px[1],
            "YUYV write_to RGB neutral-chroma R must equal G, got R={} G={}",
            px[0], px[1]
        );
        assert_eq!(
            px[1], px[2],
            "YUYV write_to RGB neutral-chroma G must equal B, got G={} B={}",
            px[1], px[2]
        );
    }
}

#[test]
fn yuyv_into_rgba_write_to_appends_opaque_alpha() {
    let data = vec![128u8; 8];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 4];
    frame
        .into_rgba()
        .write_to(&mut dest)
        .expect("YUYV write_to RGBA");
    for px in dest.as_chunks::<4>().0 {
        assert_eq!(
            px[3], 255,
            "YUYV write_to RGBA alpha must be 255, got {}",
            px[3]
        );
    }
}

// `buf_yuyv422_to_rgb` (`nokhwa-core/src/types.rs:1712-1740`) guards
// both the input divisibility (`data.len() % 4 == 0`) and the dest
// buffer size (`dest.len() == (data.len() / 4) * 2 * pixel_size`)
// before dispatching into `crate::simd::yuyv_to_{rgb,rgba}_simd`.
// The happy paths are pinned by
// `yuyv_into_rgb_write_to_neutral_chroma_produces_gray` and
// `yuyv_into_rgba_write_to_appends_opaque_alpha` above. A regression
// that dropped either guard (e.g. a refactor that consolidated YUYV +
// NV12 into a shared helper and lost the `dest.len()` check on the
// YUYV branch) would surface as a SIMD out-of-bounds panic or silent
// truncation rather than a clean `ProcessFrameError`.

#[test]
fn yuyv_into_rgb_write_to_rejects_wrong_input_size() {
    // 6 bytes is not divisible by 4 — input guard fires.
    let data = vec![100u8, 128, 200, 128, 50, 128];
    let buf = Buffer::new(Resolution::new(2, 1), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 3];
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::YUYV,
        "RGB888",
        "Assertion failure, the YUV stream isn't 4:2:2! (wrong number of bytes)",
    );
}

#[test]
fn yuyv_into_rgb_write_to_rejects_mismatched_dest() {
    // 8 bytes is a valid 2x2 YUYV stream. Expected RGB dest = 12 bytes;
    // we pass 9 to trip the dest-size guard.
    let data = vec![100u8, 128, 200, 128, 50, 128, 150, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 9];
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::YUYV,
        "RGB888",
        "Assertion failure, the destination RGB buffer is of the wrong size! \
         [expected: 12, actual: 9]",
    );
}

#[test]
fn yuyv_into_rgba_write_to_rejects_wrong_input_size() {
    let data = vec![100u8, 128, 200, 128, 50, 128];
    let buf = Buffer::new(Resolution::new(2, 1), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 4];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::YUYV,
        "RGB888",
        "Assertion failure, the YUV stream isn't 4:2:2! (wrong number of bytes)",
    );
}

#[test]
fn yuyv_into_rgba_write_to_rejects_mismatched_dest() {
    // 8 bytes is a valid 2x2 YUYV stream. Expected RGBA dest = 16 bytes;
    // we pass 12 to trip the dest-size guard.
    let data = vec![100u8, 128, 200, 128, 50, 128, 150, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 12];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::YUYV,
        "RGB888",
        "Assertion failure, the destination RGB buffer is of the wrong size! \
         [expected: 16, actual: 12]",
    );
}

// ---------------------------------------------------------------------------
// Luma conversion
// ---------------------------------------------------------------------------

#[test]
fn gray_into_luma_passthrough() {
    let data = vec![50, 100, 150, 200]; // 2x2 gray
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let frame: Frame<Gray> = Frame::new(buf);
    let img = frame.into_luma().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [50]);
    assert_eq!(img.get_pixel(1, 1).0, [200]);
}

#[test]
fn rawrgb_into_luma_averages() {
    // RGB (30, 60, 90) -> avg = 60
    let data = vec![30, 60, 90, 30, 60, 90, 30, 60, 90, 30, 60, 90];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let img = frame.into_luma().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [60]);
}

// `Frame<RawBgr>::into_luma` routes through the same
// `RAWRGB | RAWBGR` arm of `convert_to_luma{,_buffer}` because
// (B+G+R)/3 == (R+G+B)/3, but the BGR side had no end-to-end
// coverage. These four tests mirror the RAWRGB suite so a future
// refactor that splits the arms (e.g. swizzling first then averaging)
// is caught at the test layer rather than as confusing camera output.

#[test]
fn rawbgr_into_luma_averages() {
    // BGR (90, 60, 30) -> avg = 60. Same per-pixel mean as the
    // symmetric RAWRGB test, just with B/R swapped at the source.
    let data = vec![90, 60, 30, 90, 60, 30, 90, 60, 30, 90, 60, 30];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let img = frame.into_luma().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [60]);
    assert_eq!(img.get_pixel(1, 1).0, [60]);
}

#[test]
fn rawbgr_into_luma_write_to_writes_correct_averages() {
    // Per-pixel-distinct B/G/R values so the test fails loudly if the
    // SIMD kernel were ever to swizzle channels — only the mean must
    // match, but that mean must come from this exact triple.
    // Pixel 0: BGR (10, 20, 30) -> avg = 20
    // Pixel 1: BGR (40, 80, 120) -> avg = 80
    let data = vec![10, 20, 30, 40, 80, 120];
    let buf = Buffer::new(Resolution::new(2, 1), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let mut dest = vec![0u8; 2];
    frame.into_luma().write_to(&mut dest).unwrap();
    assert_eq!(dest, vec![20, 80]);
}

// `LumaConversion::write_to` for RAWRGB and GRAY: matching happy-path
// pins for the existing RAWBGR happy-path
// (`rawbgr_into_luma_write_to_writes_correct_averages`). RAWRGB
// shares the `RAWRGB | RAWBGR` averaging arm but its happy-path
// `write_to` was uncovered. GRAY's happy-path `write_to` is a
// straight `dest.copy_from_slice(...)` and was uncovered too — only
// the mismatched-dest guard was pinned. A regression that swaps the
// destination orientation (e.g. accidentally striding by 3 bytes
// instead of 1 for GRAY) or zeroes the output would slip past the
// materialize-only suite and silently break grayscale streaming.

#[test]
fn rawrgb_into_luma_write_to_writes_correct_averages() {
    // Pixel 0: RGB (30, 60, 90)   -> avg = 60
    // Pixel 1: RGB (10, 20, 30)   -> avg = 20
    let data = vec![30u8, 60, 90, 10, 20, 30];
    let buf = Buffer::new(Resolution::new(2, 1), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 2];
    frame
        .into_luma()
        .write_to(&mut dest)
        .expect("RAWRGB write_to luma");
    assert_eq!(dest, vec![60, 20]);
}

#[test]
fn gray_into_luma_write_to_passthrough() {
    // GRAY → luma is a verbatim `dest.copy_from_slice(data)` — pin
    // every byte position so a stride / orientation regression in
    // the buffer arm of `convert_to_luma_buffer` is caught.
    let data = vec![50u8, 100, 150, 200];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let frame: Frame<Gray> = Frame::new(buf);
    let mut dest = vec![0u8; 4];
    frame
        .into_luma()
        .write_to(&mut dest)
        .expect("GRAY write_to luma");
    assert_eq!(
        dest, data,
        "GRAY write_to luma must copy the byte plane verbatim"
    );
}

#[test]
fn rawbgr_into_luma_rejects_length_resolution_mismatch() {
    // 4-byte input for a 1x1 frame: the RAWRGB|RAWBGR arm's length guard
    // (`data.len() == width*height*3`) must reject it. fcc must be
    // RAWBGR — not RAWRGB — because the error reports the actual buffer
    // format, not a fallback.
    let data = vec![1, 2, 3, 4];
    let buf = Buffer::new(Resolution::new(1, 1), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let err = frame.into_luma().materialize().unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWBGR,
        "Luma",
        "RGB/BGR data length 4 does not match resolution 3",
    );
}

#[test]
fn rawbgr_into_luma_write_to_rejects_mismatched_dest() {
    // 4 pixels (12 bytes) -> 4 luma bytes expected; pass a 3-byte dest
    // to hit the `dest.len() != pixel_count` guard at frame.rs:649.
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let mut dest = vec![0u8; 3];
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWBGR,
        "Luma",
        "destination buffer size mismatch (expected 4, got 3)",
    );
}

// ---------------------------------------------------------------------------
// YUYV luma extraction (direct Y channel)
// ---------------------------------------------------------------------------

#[test]
fn yuyv_into_luma_extracts_y() {
    // YUYV: [Y0=100, U=128, Y1=200, V=128] for 2 pixels
    // 2x2 image needs 2 YUYV chunks (4 pixels = 2 chunks of 4 bytes)
    let data = vec![100, 128, 200, 128, 50, 128, 150, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let img = frame.into_luma().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [100]);
    assert_eq!(img.get_pixel(1, 0).0, [200]);
    assert_eq!(img.get_pixel(0, 1).0, [50]);
    assert_eq!(img.get_pixel(1, 1).0, [150]);
}

// ---------------------------------------------------------------------------
// NV12 luma extraction (direct Y plane copy)
// ---------------------------------------------------------------------------

#[test]
fn nv12_into_luma_extracts_y_plane() {
    // 2x2 NV12: Y plane = 4 bytes, UV plane = 2 bytes (1 pair)
    let y_plane = [10u8, 20, 30, 40];
    let uv_plane = [128u8, 128]; // neutral chroma
    let mut data = Vec::new();
    data.extend_from_slice(&y_plane);
    data.extend_from_slice(&uv_plane);

    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let img = frame.into_luma().materialize().unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [10]);
    assert_eq!(img.get_pixel(1, 0).0, [20]);
    assert_eq!(img.get_pixel(0, 1).0, [30]);
    assert_eq!(img.get_pixel(1, 1).0, [40]);
}

// `Frame<Nv12>::into_luma().write_to(...)` routes through
// `convert_to_luma_buffer`'s `FrameFormat::NV12 =>
// buf_nv12_extract_luma(...)` arm. The materialize path is pinned by
// `nv12_into_luma_extracts_y_plane` above, but the `write_to`
// integration — including both error guards in
// `buf_nv12_extract_luma` (input size mismatch, dest size mismatch)
// — had zero coverage. A regression in either guard would slip
// through CI and surface only when downstream code passes a
// pre-allocated buffer.

#[test]
fn nv12_into_luma_write_to_extracts_y_plane() {
    let y_plane = [10u8, 20, 30, 40];
    let uv_plane = [128u8, 128];
    let mut data = Vec::new();
    data.extend_from_slice(&y_plane);
    data.extend_from_slice(&uv_plane);

    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 4];
    frame.into_luma().write_to(&mut dest).unwrap();
    assert_eq!(
        dest, y_plane,
        "NV12 write_to luma must copy the Y plane verbatim, ignoring \
         the trailing UV plane",
    );
}

#[test]
fn nv12_into_luma_write_to_rejects_wrong_input_size() {
    // 2×2 NV12 needs `2*2 + 2*2/2 = 6` bytes. Pass 5 to hit the
    // input-size guard at types.rs:1893.
    let data = vec![10u8, 20, 30, 40, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 4];
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::NV12,
        "Luma",
        "NV12 input size mismatch (expected 6, got 5)",
    );
}

#[test]
fn nv12_into_luma_write_to_rejects_mismatched_dest() {
    let y_plane = [10u8, 20, 30, 40];
    let uv_plane = [128u8, 128];
    let mut data = Vec::new();
    data.extend_from_slice(&y_plane);
    data.extend_from_slice(&uv_plane);

    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::NV12);
    let frame: Frame<Nv12> = Frame::new(buf);
    let mut dest = vec![0u8; 3]; // expected y_size = 4
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::NV12,
        "Luma",
        "destination buffer size mismatch (expected 4, got 3)",
    );
}

// ---------------------------------------------------------------------------
// Luma write_to
// ---------------------------------------------------------------------------

#[test]
fn gray_luma_write_to() {
    let data = vec![10, 20, 30, 40];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let frame: Frame<Gray> = Frame::new(buf);
    let mut dest = vec![0u8; 4];
    frame.into_luma().write_to(&mut dest).unwrap();
    assert_eq!(dest, data);
}

#[test]
fn yuyv_luma_write_to() {
    let data = vec![100, 128, 200, 128, 50, 128, 150, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 4];
    frame.into_luma().write_to(&mut dest).unwrap();
    assert_eq!(dest, [100, 200, 50, 150]);
}

// `buf_yuyv_extract_luma` (`nokhwa-core/src/types.rs:1848-1872`) guards
// two pre-conditions before the SIMD copy:
//   1. `data.len() % 4 == 0` — YUYV is [Y0, U, Y1, V] quads
//   2. `dest.len() == data.len() / 2` — one luma byte per pixel
//
// NV12's symmetric guards are pinned by
// `nv12_into_luma_write_to_rejects_wrong_input_size` and
// `nv12_into_luma_write_to_rejects_mismatched_dest`. YUYV had only
// the happy-path `yuyv_luma_write_to` above. A regression that
// dropped either guard would surface as either a `crate::simd`
// out-of-bounds panic or silent truncation/garbage in the dest
// buffer — neither produces a clean `ProcessFrameError`.

#[test]
fn yuyv_into_luma_write_to_rejects_wrong_input_size() {
    // 6 bytes is not divisible by 4. We must construct a Buffer with
    // that exact length and `FrameFormat::YUYV` so the guard at
    // types.rs:1849-1853 fires before SIMD touches the slice.
    let data = vec![100u8, 128, 200, 128, 50, 128];
    let buf = Buffer::new(Resolution::new(2, 1), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    // `dest` size doesn't matter — the input-length guard fires first.
    let mut dest = vec![0u8; 4];
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::YUYV,
        "Luma",
        "YUYV stream length not divisible by 4",
    );
}

#[test]
fn yuyv_into_luma_write_to_rejects_mismatched_dest() {
    // 8 bytes is a valid YUYV stream (2x2). The input guard passes;
    // the dest guard at types.rs:1858-1867 must reject a wrongly-
    // sized dest (3 bytes instead of pixel_count = 4).
    let data = vec![100u8, 128, 200, 128, 50, 128, 150, 128];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::YUYV);
    let frame: Frame<Yuyv> = Frame::new(buf);
    let mut dest = vec![0u8; 3];
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::YUYV,
        "Luma",
        "destination buffer size mismatch (expected 4, got 3)",
    );
}

// ---------------------------------------------------------------------------
// MJPEG conversion (requires "mjpeg" feature, not WASM)
// ---------------------------------------------------------------------------

/// A valid 2×2 solid-red JPEG (quality 100, generated by `ImageMagick`).
///
/// After JPEG round-trip through YCbCr, the decoded RGB values will be close
/// to (255, 0, 0) but not exact due to lossy compression.
#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
const JPEG_RED_2X2: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0xff, 0xdb, 0x00, 0x43, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0xff, 0xc0,
    0x00, 0x11, 0x08, 0x00, 0x02, 0x00, 0x02, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11,
    0x01, 0xff, 0xc4, 0x00, 0x14, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0xff, 0xc4, 0x00, 0x14, 0x10, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xc4, 0x00,
    0x15, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x09, 0x0a, 0xff, 0xc4, 0x00, 0x14, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x01,
    0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00, 0x17, 0xc5, 0x3a, 0xfe, 0x1f, 0xff, 0xd9,
];

/// Helper: assert all pixel channels are within `tolerance` of expected.
/// The `channels` parameter is used only for diagnostic formatting (pixel index / channel id).
#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
fn assert_pixels_near(actual: &[u8], expected: &[u8], channels: usize, tolerance: u8) {
    assert_ne!(channels, 0, "channels must be non-zero");
    assert_eq!(
        actual.len(),
        expected.len(),
        "pixel data length mismatch: got {} expected {}",
        actual.len(),
        expected.len()
    );
    for (i, (&a, &e)) in actual.iter().zip(expected.iter()).enumerate() {
        let diff = a.abs_diff(e);
        assert!(
            diff <= tolerance,
            "channel {} of pixel {}: got {a}, expected {e} (diff {diff} > tolerance {tolerance})",
            i % channels,
            i / channels,
        );
    }
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_into_rgb_produces_correct_output() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let img = frame.into_rgb().materialize().unwrap();
    assert_eq!(img.width(), 2);
    assert_eq!(img.height(), 2);
    // All 4 pixels should be close to red (255, 0, 0).
    // JPEG YCbCr round-trip introduces small errors.
    let expected = [255, 0, 0].repeat(4);
    assert_pixels_near(img.as_raw(), &expected, 3, 5);
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_into_rgba_produces_correct_output() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let img = frame.into_rgba().materialize().unwrap();
    assert_eq!(img.width(), 2);
    assert_eq!(img.height(), 2);
    // All 4 pixels should be close to red with full alpha.
    // Alpha is always exactly 255 (not lossy); the tolerance applies uniformly to all
    // channels but alpha matches exactly since expected == actual == 255.
    let expected = [255, 0, 0, 255].repeat(4);
    assert_pixels_near(img.as_raw(), &expected, 4, 5);
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_into_luma_produces_correct_output() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let img = frame.into_luma().materialize().unwrap();
    assert_eq!(img.width(), 2);
    assert_eq!(img.height(), 2);
    // Luma = (R+G+B)/3. For near-red (≈255,0,0) that's ≈85.
    let expected = [85u8; 4];
    assert_pixels_near(img.as_raw(), &expected, 1, 5);
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_rgb_write_to() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 3];
    frame.into_rgb().write_to(&mut dest).unwrap();
    // Same tolerance check as materialize — near-red pixels
    let expected = [255, 0, 0].repeat(4);
    assert_pixels_near(&dest, &expected, 3, 5);
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_rgba_write_to() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2 * 4];
    frame.into_rgba().write_to(&mut dest).unwrap();
    let expected = [255, 0, 0, 255].repeat(4);
    assert_pixels_near(&dest, &expected, 4, 5);
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_luma_write_to() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 2 * 2];
    frame.into_luma().write_to(&mut dest).unwrap();
    let expected = [85u8; 4];
    assert_pixels_near(&dest, &expected, 1, 5);
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_luma_write_to_rejects_mismatched_dest() {
    // `convert_to_luma_buffer`'s MJPEG arm now requires an exact-length
    // destination (same contract as GRAY / RAW{RGB,BGR}). Both too-small
    // and too-large dests must return `ProcessFrameError` with a
    // "destination buffer size mismatch" message.
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf.clone());
    let mut too_small = vec![0u8; 3]; // expected = 4 (2x2 = 4 luma bytes)
    let err = frame.into_luma().write_to(&mut too_small).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::MJPEG,
        "Luma",
        "destination buffer size mismatch (expected 4, got 3)",
    );

    let frame2: Frame<Mjpeg> = Frame::new(buf);
    let mut too_large = vec![0u8; 8]; // expected = 4; oversized is now also rejected
    let err2 = frame2.into_luma().write_to(&mut too_large).unwrap_err();
    assert_process_frame_err(
        err2,
        FrameFormat::MJPEG,
        "Luma",
        "destination buffer size mismatch (expected 4, got 8)",
    );
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_malformed_returns_error() {
    // Starts with valid JPEG SOI marker but truncated. The previous
    // test only checked `is_err()`, so a regression that swapped the
    // variant (e.g. wrapping the mozjpeg failure in `GeneralError`)
    // or routed the error through a non-MJPEG `src` would have
    // passed. Pin the variant + src + destination from `mjpeg_to_rgb`
    // (`nokhwa-core/src/types.rs:1543-1593`); leave the inner `error`
    // field unpinned because it is mozjpeg's own diagnostic text and
    // can drift across mozjpeg versions — assert only that it is
    // non-empty.
    let garbage = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x00, 0x00];
    let buf = Buffer::new(Resolution::new(2, 2), garbage, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let err = frame.into_rgb().materialize().unwrap_err();
    match err {
        NokhwaError::ProcessFrameError {
            src,
            destination,
            error,
        } => {
            assert_eq!(src, FrameFormat::MJPEG);
            assert_eq!(destination, "RGB888");
            assert!(!error.is_empty(), "mozjpeg error message must not be empty");
        },
        other => panic!("expected ProcessFrameError, got {other:?}"),
    }
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_empty_returns_error() {
    // Mirror the malformed-input pin above for the empty-buffer path.
    // Both fail at the `Decompress::new_mem` step in
    // `mjpeg_to_rgb` (`nokhwa-core/src/types.rs:1546`), so the routed
    // (src, destination) pair is the same.
    let buf = Buffer::new(Resolution::new(2, 2), &[], FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let err = frame.into_rgb().materialize().unwrap_err();
    match err {
        NokhwaError::ProcessFrameError {
            src,
            destination,
            error,
        } => {
            assert_eq!(src, FrameFormat::MJPEG);
            assert_eq!(destination, "RGB888");
            assert!(!error.is_empty(), "mozjpeg error message must not be empty");
        },
        other => panic!("expected ProcessFrameError, got {other:?}"),
    }
}

// `RgbConversion::write_to` and `RgbaConversion::write_to` for
// `Frame<Mjpeg>` delegate straight to `buf_mjpeg_to_rgb` (types.rs)
// with no upper-layer dest-size check (the predicted size requires
// decoding the JPEG header). The decoder asserts
// `dest.len() == jpeg_decompress.min_flat_buffer_size()` and errors
// with `"Bad decoded buffer size"` otherwise. Pin both ends of the
// guard so a refactor that rounds the comparison or drops it would
// surface as a panic-on-OOB instead of a clean `ProcessFrameError`.

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_rgb_write_to_rejects_too_small_dest() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 11]; // expected 12 (2x2 RGB)
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::MJPEG, "RGB888", "Bad decoded buffer size");
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_rgb_write_to_rejects_too_large_dest() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 13]; // expected 12
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::MJPEG, "RGB888", "Bad decoded buffer size");
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_rgba_write_to_rejects_too_small_dest() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 15]; // expected 16 (2x2 RGBA)
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::MJPEG, "RGB888", "Bad decoded buffer size");
}

#[cfg(all(feature = "mjpeg", not(target_arch = "wasm32")))]
#[test]
fn mjpeg_rgba_write_to_rejects_too_large_dest() {
    let buf = Buffer::new(Resolution::new(2, 2), JPEG_RED_2X2, FrameFormat::MJPEG);
    let frame: Frame<Mjpeg> = Frame::new(buf);
    let mut dest = vec![0u8; 17]; // expected 16
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(err, FrameFormat::MJPEG, "RGB888", "Bad decoded buffer size");
}

// ---------------------------------------------------------------------------
// `write_to` destination-buffer guards
//
// `convert_to_{rgb,rgba,luma}_buffer` reject mismatched destination buffer
// sizes and non-multiple-of-3 RAWRGB/RAWBGR data. Previously every test
// passed a correctly-sized destination, so those guards were uncovered —
// a regression in the size-check arithmetic would not have failed CI.
// ---------------------------------------------------------------------------

// Exact-string matcher (rather than `error.contains(needle)`) so a regression
// that changes the error wording — or drops a computed `expected={N}, got={M}`
// suffix — is caught by CI. Every caller below pins the verbatim message
// produced by the source-of-truth at the matching site in `frame.rs` /
// `types.rs` / `mjpeg.rs`.
fn assert_process_frame_err(
    err: NokhwaError,
    expected_src: FrameFormat,
    expected_dst: &str,
    expected_error: &str,
) {
    match err {
        NokhwaError::ProcessFrameError {
            src,
            destination,
            error,
        } => {
            assert_eq!(src, expected_src);
            assert_eq!(destination, expected_dst);
            assert_eq!(error, expected_error);
        },
        other => panic!("expected ProcessFrameError, got {other:?}"),
    }
}

#[test]
fn rawrgb_into_rgb_write_to_rejects_mismatched_dest() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 11]; // off by one
    let err = frame.into_rgb().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "RGB",
        "destination buffer size mismatch (expected 12, got 11)",
    );
}

// `Frame<Gray>` does not implement `IntoRgb`/`IntoRgba` (gray is luma-only),
// so the GRAY-branch guards in `convert_to_rgb_buffer` / `convert_to_rgba_buffer`
// are reachable only through the crate-internal dispatcher.
#[test]
fn convert_to_rgb_buffer_gray_rejects_mismatched_dest() {
    let data = vec![10u8, 20, 30, 40];
    let mut dest = vec![0u8; 11]; // expected 4 * 3 = 12
    let err = convert_to_rgb_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .unwrap_err();
    assert_process_frame_err(err, FrameFormat::GRAY, "RGB", "Bad buffer length");
}

#[test]
fn rawrgb_into_rgb_rejects_length_resolution_mismatch() {
    // `convert_to_rgb`'s RAWRGB arm guards `data.len() == width*height*3`,
    // matching the GRAY arm and the `buf_bgr_to_rgb` RAWBGR path. A
    // non-`w*h*3` length (here also not a multiple of 3) is rejected.
    let data = vec![1, 2, 3, 4]; // 4 bytes, but 1x1 RAWRGB expects 3
    let err = convert_to_rgb(FrameFormat::RAWRGB, Resolution::new(1, 1), &data).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "RGB",
        "RAWRGB data length 4 does not match resolution 3",
    );
}

#[test]
fn rawrgb_into_rgba_rejects_length_resolution_mismatch() {
    let data = vec![1, 2, 3, 4]; // 4 bytes, but 1x1 RAWRGB expects 3
    let buf = Buffer::new(Resolution::new(1, 1), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let err = frame.into_rgba().materialize().unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "RGBA",
        "RAWRGB data length 4 does not match resolution 3",
    );
}

#[test]
fn rawbgr_into_rgba_rejects_length_resolution_mismatch() {
    let data = vec![1, 2, 3, 4, 5]; // 5 bytes, but 1x1 RAWBGR expects 3
    let buf = Buffer::new(Resolution::new(1, 1), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let err = frame.into_rgba().materialize().unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWBGR,
        "RGBA",
        "RAWBGR data length 5 does not match resolution 3",
    );
}

#[test]
fn rawrgb_into_rgba_write_to_rejects_length_resolution_mismatch() {
    let data = vec![1, 2, 3, 4]; // 4 bytes, but 1x1 RAWRGB expects 3
    let buf = Buffer::new(Resolution::new(1, 1), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 4];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "RGBA",
        "RAWRGB data length 4 does not match resolution 3",
    );
}

#[test]
fn rawrgb_into_rgba_write_to_rejects_mismatched_dest() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]; // 12 bytes -> 16 RGBA
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 15]; // expected 16
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "RGBA",
        "destination buffer size mismatch (expected 16, got 15)",
    );
}

#[test]
fn rawbgr_into_rgba_write_to_rejects_mismatched_dest() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWBGR);
    let frame: Frame<RawBgr> = Frame::new(buf);
    let mut dest = vec![0u8; 15];
    let err = frame.into_rgba().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWBGR,
        "RGBA",
        "destination buffer size mismatch (expected 16, got 15)",
    );
}

#[test]
fn convert_to_rgba_buffer_gray_rejects_mismatched_dest() {
    let data = vec![10u8, 20, 30, 40];
    let mut dest = vec![0u8; 15]; // expected 4 * 4 = 16
    let err = convert_to_rgba_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .unwrap_err();
    assert_process_frame_err(err, FrameFormat::GRAY, "RGBA", "Bad buffer length");
}

// `Frame<Gray>` does not implement `IntoRgb` / `IntoRgba` (luma-only
// at the type level), so the GRAY happy paths in `convert_to_rgb`,
// `convert_to_rgb_buffer`, `convert_to_rgba`, and
// `convert_to_rgba_buffer` are reachable only via the
// crate-internal dispatcher. The reject-paths are pinned above;
// without these, a regression that swaps the channel-replication
// (e.g. `[pxv, 0, pxv]`) or hard-codes the wrong alpha (e.g.
// `[pxv, pxv, pxv, 0]` instead of 255) would silently corrupt
// every monochrome capture on a downstream RGB / RGBA conversion.

#[test]
fn convert_to_rgb_gray_replicates_luma_to_rgb_triplet() {
    // GRAY → RGB expands each luma byte to `[Y, Y, Y]`. Pin the
    // replication shape so a future `[pxv, 0, pxv]` typo or
    // off-by-one chunk stride is caught.
    let data = vec![10u8, 50, 200, 255];
    let rgb = convert_to_rgb(FrameFormat::GRAY, Resolution::new(2, 2), &data)
        .expect("GRAY → RGB happy path");
    assert_eq!(rgb.len(), data.len() * 3);
    assert_eq!(
        rgb,
        vec![10, 10, 10, 50, 50, 50, 200, 200, 200, 255, 255, 255]
    );
}

#[test]
fn convert_to_rgb_buffer_gray_writes_luma_triplet_to_dest() {
    // Same replication contract as the `Vec`-returning variant
    // but writing into a caller-owned buffer. Pinned so a
    // regression that uses `chunks_exact_mut(4)` instead of
    // index-based writing for RGB is caught.
    let data = vec![10u8, 50, 200, 255];
    let mut dest = vec![0u8; data.len() * 3];
    convert_to_rgb_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .expect("GRAY → RGB buffer happy path");
    assert_eq!(
        dest,
        vec![10, 10, 10, 50, 50, 50, 200, 200, 200, 255, 255, 255]
    );
}

#[test]
fn convert_to_rgba_gray_replicates_luma_with_opaque_alpha() {
    // GRAY → RGBA expands each luma byte to `[Y, Y, Y, 255]`.
    // The alpha channel is **always** 255 — pin so a future
    // tweak that uses 0 (transparent) or `pxv` (luma-as-alpha)
    // surfaces here, not as invisible monochrome frames in user
    // applications.
    let data = vec![10u8, 50, 200, 255];
    let rgba = convert_to_rgba(FrameFormat::GRAY, Resolution::new(2, 2), &data)
        .expect("GRAY → RGBA happy path");
    assert_eq!(rgba.len(), data.len() * 4);
    assert_eq!(
        rgba,
        vec![
            10, 10, 10, 255, //
            50, 50, 50, 255, //
            200, 200, 200, 255, //
            255, 255, 255, 255,
        ]
    );
}

#[test]
fn convert_to_rgba_buffer_gray_writes_luma_with_opaque_alpha() {
    // Same `[Y, Y, Y, 255]` contract but to a caller-owned
    // dest. Indexes into `dest[i+3] = 255` directly; pin so a
    // regression to `dest[i+3] = pxv` doesn't sneak through
    // unnoticed.
    let data = vec![10u8, 50, 200, 255];
    let mut dest = vec![0u8; data.len() * 4];
    convert_to_rgba_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .expect("GRAY → RGBA buffer happy path");
    assert_eq!(
        dest,
        vec![
            10, 10, 10, 255, //
            50, 50, 50, 255, //
            200, 200, 200, 255, //
            255, 255, 255, 255,
        ]
    );
}

#[test]
fn rawrgb_into_luma_rejects_length_resolution_mismatch() {
    let data = vec![1, 2, 3, 4];
    let buf = Buffer::new(Resolution::new(1, 1), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let err = frame.into_luma().materialize().unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "Luma",
        "RGB/BGR data length 4 does not match resolution 3",
    );
}

#[test]
fn rawrgb_into_luma_write_to_rejects_length_resolution_mismatch() {
    let data = vec![1, 2, 3, 4];
    let buf = Buffer::new(Resolution::new(1, 1), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 1];
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "Luma",
        "RGB/BGR data length 4 does not match resolution 3",
    );
}

#[test]
fn rawrgb_into_luma_write_to_rejects_mismatched_dest() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]; // 4 pixels -> 4 luma
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::RAWRGB);
    let frame: Frame<RawRgb> = Frame::new(buf);
    let mut dest = vec![0u8; 3]; // expected 4
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "Luma",
        "destination buffer size mismatch (expected 4, got 3)",
    );
}

#[test]
fn gray_into_luma_write_to_rejects_mismatched_dest() {
    let data = vec![10, 20, 30, 40];
    let buf = Buffer::new(Resolution::new(2, 2), &data, FrameFormat::GRAY);
    let frame: Frame<Gray> = Frame::new(buf);
    let mut dest = vec![0u8; 3]; // expected 4
    let err = frame.into_luma().write_to(&mut dest).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::GRAY,
        "Luma",
        "destination buffer size mismatch (expected 4, got 3)",
    );
}

// Gap A: convert_to_rgb GRAY arm was missing a resolution vs. data.len() check.
// A Buffer built with a mismatched resolution (e.g. 4×4 = 16 expected but only
// 10 bytes of data) would silently produce a wrong-pixel-count output.
// Every other arm validates input size against resolution — now GRAY does too.
#[test]
fn convert_to_rgb_gray_rejects_resolution_mismatch() {
    // Resolution says 4×4 = 16 pixels, but only 10 bytes of data.
    // Before the fix this would silently produce a 30-byte (10*3) output
    // instead of the expected 48-byte (16*3) one.
    let data = vec![128u8; 10];
    let err = convert_to_rgb(FrameFormat::GRAY, Resolution::new(4, 4), &data).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::GRAY,
        "RGB",
        "GRAY data length 10 does not match resolution 16",
    );
}

// convert_to_rgb_buffer's RAWRGB arm validates data.len() == width*height*3,
// like the GRAY arm and the buf_bgr_to_rgb RAWBGR path. A payload whose length
// does not match the resolution would otherwise be silently copied into dest.
#[test]
fn convert_to_rgb_buffer_rawrgb_rejects_length_resolution_mismatch() {
    // 7 bytes for a 1x1 frame, which expects 3.
    let data = vec![1u8, 2, 3, 4, 5, 6, 7];
    let mut dest = vec![0u8; 7]; // same length as data to avoid the dest-mismatch error
    let err = convert_to_rgb_buffer(FrameFormat::RAWRGB, Resolution::new(1, 1), &data, &mut dest)
        .unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::RAWRGB,
        "RGB",
        "RAWRGB data length 7 does not match resolution 3",
    );
}

// ---------------------------------------------------------------------------
// C1a: convert_to_rgb_buffer GRAY — data.len() != w*h guard
// ---------------------------------------------------------------------------

// Before the fix, a GRAY buffer whose length didn't match the declared
// resolution would skip the w*h check and land directly in the dest-size
// check (`dest.len() != data.len() * 3`), silently producing wrong-
// dimensioned output. Now the w*h guard fires first.

#[test]
fn convert_to_rgb_buffer_gray_rejects_resolution_mismatch() {
    // Resolution 2×2 expects 4 bytes; supply 3 — the w*h guard must fire.
    let data = vec![10u8, 20, 30];
    let mut dest = vec![0u8; 9]; // 3 * 3 = 9 would satisfy the OLD dest check
    let err = convert_to_rgb_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::GRAY,
        "RGB",
        "GRAY data length 3 does not match resolution 4",
    );
}

#[test]
fn convert_to_rgb_buffer_gray_correct_size_ok() {
    // 2×2 GRAY → RGB: data.len() == 4 == w*h, dest.len() == 12 == 4*3. Must succeed.
    let data = vec![10u8, 50, 200, 255];
    let mut dest = vec![0u8; 12];
    convert_to_rgb_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .expect("GRAY rgb_buffer happy path");
    assert_eq!(
        dest,
        vec![10, 10, 10, 50, 50, 50, 200, 200, 200, 255, 255, 255]
    );
}

// ---------------------------------------------------------------------------
// C1b: convert_to_rgba GRAY — data.len() != w*h guard
// ---------------------------------------------------------------------------

#[test]
fn convert_to_rgba_gray_rejects_resolution_mismatch() {
    // Resolution 2×2 expects 4 bytes; supply 5.
    let data = vec![10u8, 20, 30, 40, 50];
    let err = convert_to_rgba(FrameFormat::GRAY, Resolution::new(2, 2), &data).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::GRAY,
        "RGBA",
        "GRAY data length 5 does not match resolution 4",
    );
}

#[test]
fn convert_to_rgba_gray_correct_size_ok() {
    // 2×2 GRAY → RGBA. Each pixel becomes [Y, Y, Y, 255].
    let data = vec![10u8, 50, 200, 255];
    let rgba = convert_to_rgba(FrameFormat::GRAY, Resolution::new(2, 2), &data)
        .expect("GRAY rgba happy path");
    assert_eq!(rgba.len(), 16);
    assert_eq!(
        rgba,
        vec![10, 10, 10, 255, 50, 50, 50, 255, 200, 200, 200, 255, 255, 255, 255, 255]
    );
}

// ---------------------------------------------------------------------------
// C1b also: convert_to_rgba_buffer GRAY — data.len() != w*h guard
// ---------------------------------------------------------------------------

#[test]
fn convert_to_rgba_buffer_gray_rejects_resolution_mismatch() {
    // Resolution 2×2 expects 4 bytes; supply 6.
    let data = vec![10u8, 20, 30, 40, 50, 60];
    let mut dest = vec![0u8; 24]; // 6 * 4 — would pass OLD dest check
    let err = convert_to_rgba_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::GRAY,
        "RGBA",
        "GRAY data length 6 does not match resolution 4",
    );
}

#[test]
fn convert_to_rgba_buffer_gray_correct_size_ok() {
    // 2×2 GRAY → RGBA buffer. Each pixel becomes [Y, Y, Y, 255].
    let data = vec![10u8, 50, 200, 255];
    let mut dest = vec![0u8; 16];
    convert_to_rgba_buffer(FrameFormat::GRAY, Resolution::new(2, 2), &data, &mut dest)
        .expect("GRAY rgba_buffer happy path");
    assert_eq!(
        dest,
        vec![10, 10, 10, 255, 50, 50, 50, 255, 200, 200, 200, 255, 255, 255, 255, 255]
    );
}

// ---------------------------------------------------------------------------
// C2: convert_to_luma GRAY — data.len() != w*h guard
// ---------------------------------------------------------------------------

// Before the fix, GRAY luma passthrough (`Ok(data.to_vec())`) had no size
// validation at all. A mismatched-length buffer would silently return a
// Vec with the wrong number of pixels, causing ImageBuffer::from_raw to
// fail or to accept wrong-dimensioned data.

#[test]
fn convert_to_luma_gray_rejects_resolution_mismatch() {
    // Resolution 3×3 expects 9 bytes; supply 7.
    let data = vec![10u8; 7];
    let err = convert_to_luma(FrameFormat::GRAY, Resolution::new(3, 3), &data).unwrap_err();
    assert_process_frame_err(
        err,
        FrameFormat::GRAY,
        "Luma",
        "GRAY data length 7 does not match resolution 9",
    );
}

#[test]
fn convert_to_luma_gray_correct_size_ok() {
    // 2×2 GRAY → Luma: passthrough, same bytes.
    let data = vec![10u8, 50, 200, 255];
    let luma = convert_to_luma(FrameFormat::GRAY, Resolution::new(2, 2), &data)
        .expect("GRAY luma happy path");
    assert_eq!(luma, data);
}
