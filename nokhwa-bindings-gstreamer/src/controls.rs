//! GStreamer backend controls.
//!
//! ## Scope
//!
//! GStreamer's source elements expose camera controls very unevenly:
//!
//! - **Linux `v4l2src`** — four `controllable` GObject properties
//!   (`brightness`, `contrast`, `hue`, `saturation`) readable + writable
//!   at any pipeline state. Other V4L2 controls (exposure, zoom, focus,
//!   pan/tilt, gain, sharpness, gamma, white-balance, backlight-comp)
//!   are reachable only through the write-only `extra-controls`
//!   `GstStructure`, which the element applies during the transition
//!   to PAUSED. We expose them via `set_control()`; reads for these
//!   controls are not supported through GStreamer — use the native
//!   `input-v4l` backend if full control introspection matters.
//! - **Windows `mfvideosrc` / `ksvideosrc`** — no camera-control
//!   properties whatsoever. `controls()` returns an empty list and
//!   `set_control()` errors. Users on Windows should use
//!   `input-msmf`.
//! - **macOS `avfvideosrc`** — treated the same as Windows until
//!   verified on real hardware.
//!
//! This module implements the Linux path; the others rely on the
//! introspection returning an empty list.

use gstreamer::glib::ParamFlags;
use gstreamer::prelude::*;
use gstreamer::Element;
use nokhwa_core::{
    error::NokhwaError,
    types::{
        CameraControl, ControlValueDescription, ControlValueSetter, KnownCameraControl,
        KnownCameraControlFlag,
    },
};
use std::collections::BTreeMap;

/// GStreamer-side name for a control that we map to a [`KnownCameraControl`].
///
/// Two kinds: `Property(name)` is a direct `controllable` GObject
/// property on the source element (live read + write). `V4l2Cid(name)`
/// is a write-only entry delivered through the `extra-controls`
/// structure on Linux `v4l2src`.
#[derive(Copy, Clone, Debug)]
pub(crate) enum GstControlHandle {
    Property(&'static str),
    V4l2Cid(&'static str),
}

/// Static map from [`KnownCameraControl`] to the GStreamer name we
/// should use. Kept in one place so the V4L2-CID spelling and the
/// property spelling stay in sync. Returns `None` for
/// `KnownCameraControl::Other(_)` — callers are expected to pass raw
/// V4L2 CIDs via `set_control_extra` if they need something this map
/// doesn't know about.
pub(crate) fn control_handle(kcc: KnownCameraControl) -> Option<GstControlHandle> {
    Some(match kcc {
        KnownCameraControl::Brightness => GstControlHandle::Property("brightness"),
        KnownCameraControl::Contrast => GstControlHandle::Property("contrast"),
        KnownCameraControl::Hue => GstControlHandle::Property("hue"),
        KnownCameraControl::Saturation => GstControlHandle::Property("saturation"),
        KnownCameraControl::Sharpness => GstControlHandle::V4l2Cid("sharpness"),
        KnownCameraControl::Gamma => GstControlHandle::V4l2Cid("gamma"),
        KnownCameraControl::WhiteBalance => GstControlHandle::V4l2Cid("white_balance_temperature"),
        KnownCameraControl::BacklightComp => GstControlHandle::V4l2Cid("backlight_compensation"),
        KnownCameraControl::Gain => GstControlHandle::V4l2Cid("gain"),
        KnownCameraControl::Pan => GstControlHandle::V4l2Cid("pan_absolute"),
        KnownCameraControl::Tilt => GstControlHandle::V4l2Cid("tilt_absolute"),
        KnownCameraControl::Zoom => GstControlHandle::V4l2Cid("zoom_absolute"),
        KnownCameraControl::Exposure => GstControlHandle::V4l2Cid("exposure_time_absolute"),
        KnownCameraControl::Iris => GstControlHandle::V4l2Cid("iris_absolute"),
        KnownCameraControl::Focus => GstControlHandle::V4l2Cid("focus_absolute"),
        KnownCameraControl::Other(_) => return None,
    })
}

/// Enumerate the live, readable controls of `source`. Returns an empty
/// `Vec` on Windows / macOS because `mfvideosrc` / `ksvideosrc` /
/// `avfvideosrc` do not expose camera-control properties.
///
/// Uses `list_properties()` to discover which of the four `v4l2src`
/// properties the element actually offers, then reads the current
/// integer value and pspec range to build a [`CameraControl`].
pub(crate) fn list_controls(source: &Element) -> Vec<CameraControl> {
    let mut out = Vec::new();
    let pspecs = source.list_properties();
    for pspec in pspecs {
        let name = pspec.name();
        let Some(kcc) = known_from_property_name(name) else {
            continue;
        };
        // Filter out read-only / write-only properties — we need both
        // sides for a meaningful `CameraControl`.
        let flags = pspec.flags();
        if !flags.contains(ParamFlags::READABLE) || !flags.contains(ParamFlags::WRITABLE) {
            continue;
        }
        let Some(control) = build_integer_control(source, name, kcc, &pspec) else {
            continue;
        };
        out.push(control);
    }
    out
}

fn known_from_property_name(name: &str) -> Option<KnownCameraControl> {
    match name {
        "brightness" => Some(KnownCameraControl::Brightness),
        "contrast" => Some(KnownCameraControl::Contrast),
        "hue" => Some(KnownCameraControl::Hue),
        "saturation" => Some(KnownCameraControl::Saturation),
        _ => None,
    }
}

fn build_integer_control(
    source: &Element,
    name: &str,
    kcc: KnownCameraControl,
    pspec: &gstreamer::glib::ParamSpec,
) -> Option<CameraControl> {
    use gstreamer::glib::ParamSpecInt;

    let int_pspec = pspec.downcast_ref::<ParamSpecInt>()?;
    let value: i32 = source.property(name);
    Some(CameraControl::new(
        kcc,
        kcc.to_string(),
        ControlValueDescription::IntegerRange {
            min: i64::from(int_pspec.minimum()),
            max: i64::from(int_pspec.maximum()),
            value: i64::from(value),
            step: 1,
            default: i64::from(int_pspec.default_value()),
        },
        vec![KnownCameraControlFlag::Manual],
        true,
    ))
}

/// Apply a control whose [`GstControlHandle`] is a live GObject
/// property (`brightness` / `contrast` / `hue` / `saturation`). Works
/// at any pipeline state.
pub(crate) fn set_live_property(
    source: &Element,
    property: &str,
    value: &ControlValueSetter,
) -> Result<(), NokhwaError> {
    let int_value = match value {
        ControlValueSetter::Integer(i) => i32::try_from(*i).map_err(|_| {
            NokhwaError::set_property(
                property.to_string(),
                i.to_string(),
                "i64 value exceeds i32 range expected by v4l2src property",
            )
        })?,
        ControlValueSetter::Boolean(b) => i32::from(*b),
        other => {
            return Err(NokhwaError::set_property(
                property.to_string(),
                other.to_string(),
                "unsupported ControlValueSetter variant for live property",
            ));
        },
    };
    source.set_property(property, int_value);
    Ok(())
}

/// Build a GstStructure usable as `v4l2src`'s `extra-controls`
/// property from a pending-controls map. Returns `Ok(None)` if the
/// map is empty so the caller can skip setting the property at all
/// (empty extra-controls is a no-op but triggers a warning).
///
/// Returns `Err` if any stored `i64` value falls outside the `i32`
/// range expected by v4l2src (V4L2 CIDs are `__s32`-valued).
pub(crate) fn build_extra_controls(
    pending: &BTreeMap<String, i64>,
) -> Result<Option<gstreamer::Structure>, NokhwaError> {
    if pending.is_empty() {
        return Ok(None);
    }
    // v4l2src expects the structure name to be "c" (as in
    // `controls`); any other name is ignored silently.
    let mut builder = gstreamer::Structure::builder("c");
    for (k, v) in pending {
        let v32 = i32::try_from(*v).map_err(|_| {
            NokhwaError::set_property(
                k.clone(),
                v.to_string(),
                "i64 value exceeds i32 range expected by v4l2src extra-controls",
            )
        })?;
        builder = builder.field(k.as_str(), v32);
    }
    Ok(Some(builder.build()))
}

/// Translate a [`ControlValueSetter`] into the integer payload used by
/// V4L2 CIDs. Bool becomes 0/1.
pub(crate) fn v4l2_cid_value(cid: &str, value: &ControlValueSetter) -> Result<i64, NokhwaError> {
    match value {
        ControlValueSetter::Integer(i) => Ok(*i),
        ControlValueSetter::Boolean(b) => Ok(i64::from(*b)),
        other => Err(NokhwaError::set_property(
            cid.to_string(),
            other.to_string(),
            "unsupported ControlValueSetter variant for V4L2 CID",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    /// `gstreamer::Structure::builder` requires the global registry to
    /// be initialised. Same `Once` guard pattern as `format::tests`.
    fn ensure_gst_init() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            gstreamer::init().expect("gstreamer::init() must succeed in tests");
        });
    }

    #[test]
    fn control_handle_brightness_is_live_property() {
        match control_handle(KnownCameraControl::Brightness) {
            Some(GstControlHandle::Property("brightness")) => {},
            other => panic!("expected Property(\"brightness\"), got {other:?}"),
        }
    }

    #[test]
    fn control_handle_contrast_is_live_property() {
        match control_handle(KnownCameraControl::Contrast) {
            Some(GstControlHandle::Property("contrast")) => {},
            other => panic!("expected Property(\"contrast\"), got {other:?}"),
        }
    }

    #[test]
    fn control_handle_hue_is_live_property() {
        match control_handle(KnownCameraControl::Hue) {
            Some(GstControlHandle::Property("hue")) => {},
            other => panic!("expected Property(\"hue\"), got {other:?}"),
        }
    }

    #[test]
    fn control_handle_saturation_is_live_property() {
        match control_handle(KnownCameraControl::Saturation) {
            Some(GstControlHandle::Property("saturation")) => {},
            other => panic!("expected Property(\"saturation\"), got {other:?}"),
        }
    }

    #[test]
    fn control_handle_extra_controls_use_v4l2_cid_names() {
        // The `extra-controls` payload is a `GstStructure` keyed by
        // V4L2 CID name strings — these spellings are part of the
        // ABI contract with `v4l2src` and silent renames will silently
        // break control writes on real hardware.
        let pairs: &[(KnownCameraControl, &str)] = &[
            (KnownCameraControl::Sharpness, "sharpness"),
            (KnownCameraControl::Gamma, "gamma"),
            (
                KnownCameraControl::WhiteBalance,
                "white_balance_temperature",
            ),
            (KnownCameraControl::BacklightComp, "backlight_compensation"),
            (KnownCameraControl::Gain, "gain"),
            (KnownCameraControl::Pan, "pan_absolute"),
            (KnownCameraControl::Tilt, "tilt_absolute"),
            (KnownCameraControl::Zoom, "zoom_absolute"),
            (KnownCameraControl::Exposure, "exposure_time_absolute"),
            (KnownCameraControl::Iris, "iris_absolute"),
            (KnownCameraControl::Focus, "focus_absolute"),
        ];
        for (kcc, expected) in pairs {
            match control_handle(*kcc) {
                Some(GstControlHandle::V4l2Cid(name)) => {
                    assert_eq!(name, *expected, "wrong CID name for {kcc:?}");
                },
                other => panic!("expected V4l2Cid for {kcc:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn control_handle_other_returns_none() {
        // `KnownCameraControl::Other(_)` is the catch-all that callers
        // must dispatch to `set_control_extra` themselves; the static
        // map deliberately doesn't know about it.
        assert!(control_handle(KnownCameraControl::Other(0xdead_beef)).is_none());
    }

    #[test]
    fn control_handle_covers_every_known_variant() {
        // Pin: every variant of `KnownCameraControl` except `Other` must
        // produce some handle. A new variant added without a `match` arm
        // will fail compilation here.
        use nokhwa_core::types::all_known_camera_controls;
        for kcc in all_known_camera_controls() {
            assert!(
                control_handle(kcc).is_some(),
                "control_handle({kcc:?}) returned None — missing match arm?"
            );
        }
    }

    #[test]
    fn known_from_property_name_maps_four_v4l2src_props() {
        assert_eq!(
            known_from_property_name("brightness"),
            Some(KnownCameraControl::Brightness)
        );
        assert_eq!(
            known_from_property_name("contrast"),
            Some(KnownCameraControl::Contrast)
        );
        assert_eq!(
            known_from_property_name("hue"),
            Some(KnownCameraControl::Hue)
        );
        assert_eq!(
            known_from_property_name("saturation"),
            Some(KnownCameraControl::Saturation)
        );
    }

    #[test]
    fn known_from_property_name_unknown_returns_none() {
        // `list_controls` skips structures whose property name doesn't
        // match — anything outside the four `v4l2src` names is "not a
        // known camera control" and must return None.
        assert!(known_from_property_name("name").is_none());
        assert!(known_from_property_name("zoom_absolute").is_none());
        assert!(known_from_property_name("").is_none());
        assert!(known_from_property_name("Brightness").is_none()); // case-sensitive
    }

    #[test]
    fn build_extra_controls_empty_returns_none() {
        // Empty pending → caller skips the property set entirely
        // (setting an empty `extra-controls` triggers a v4l2src warning
        // at PAUSED transition).
        assert!(build_extra_controls(&BTreeMap::new()).unwrap().is_none());
    }

    #[test]
    fn build_extra_controls_uses_structure_name_c() {
        ensure_gst_init();
        let mut pending = BTreeMap::new();
        pending.insert("zoom_absolute".to_string(), 5);
        let structure = build_extra_controls(&pending)
            .expect("Ok for valid values")
            .expect("Some for non-empty pending");
        // v4l2src ignores the structure if the name is anything other
        // than "c" — pin the spelling.
        assert_eq!(structure.name(), "c");
    }

    #[test]
    fn build_extra_controls_writes_field_per_entry() {
        ensure_gst_init();
        let mut pending = BTreeMap::new();
        pending.insert("zoom_absolute".to_string(), 5);
        pending.insert("focus_absolute".to_string(), 100);
        pending.insert("exposure_time_absolute".to_string(), 250);
        let structure = build_extra_controls(&pending).unwrap().unwrap();
        assert_eq!(structure.get::<i32>("zoom_absolute").unwrap(), 5);
        assert_eq!(structure.get::<i32>("focus_absolute").unwrap(), 100);
        assert_eq!(structure.get::<i32>("exposure_time_absolute").unwrap(), 250);
    }

    #[test]
    fn build_extra_controls_errors_on_i64_outside_i32_range() {
        // V4L2 CIDs are `__s32`-valued; an out-of-range i64 must now
        // produce a `SetPropertyError` naming the offending CID, the
        // raw i64, and the canonical error string rather than silently
        // wrapping/truncating.
        ensure_gst_init();
        let mut pending = BTreeMap::new();
        let bad_value = i64::from(i32::MAX) + 1;
        pending.insert("focus_absolute".to_string(), bad_value);
        let err = build_extra_controls(&pending).unwrap_err();
        match err {
            NokhwaError::SetPropertyError {
                property,
                value,
                error,
            } => {
                assert_eq!(property, "focus_absolute");
                assert_eq!(value, bad_value.to_string());
                assert_eq!(
                    error,
                    "i64 value exceeds i32 range expected by v4l2src extra-controls"
                );
            },
            other => panic!("expected SetPropertyError, got {other:?}"),
        }
    }

    #[test]
    fn v4l2_cid_value_integer_passthrough() {
        let v = v4l2_cid_value("zoom_absolute", &ControlValueSetter::Integer(42)).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn v4l2_cid_value_boolean_maps_to_zero_or_one() {
        assert_eq!(
            v4l2_cid_value("backlight_compensation", &ControlValueSetter::Boolean(true)).unwrap(),
            1
        );
        assert_eq!(
            v4l2_cid_value(
                "backlight_compensation",
                &ControlValueSetter::Boolean(false)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn v4l2_cid_value_rejects_unsupported_setters() {
        // V4L2 CIDs are int-valued — Float / String / RGB etc. must
        // fail with a SetPropertyError that pins the offending CID,
        // the rejected setter's `Display` form, and the canonical
        // explanation. The previous version of this test only checked
        // `Display::contains("focus_absolute")`, which would still
        // pass if a future refactor:
        //   - swapped the variant for `GeneralError` / `StructureError`
        //     (callers pattern-matching on `SetPropertyError` would
        //      silently break),
        //   - dropped the rejected `value` from the struct (debugging
        //     a misuse becomes "unsupported variant" with no hint of
        //     *which* variant was passed),
        //   - or rephrased the canonical "unsupported
        //     ControlValueSetter variant for V4L2 CID" string,
        //     which is documented end-user contract for the GStreamer
        //     V4L2-CID path.
        // Pin all three fields verbatim per setter so each branch is
        // covered.
        let cid = "focus_absolute";
        let unsupported_setters = [
            ControlValueSetter::Float(1.5),
            ControlValueSetter::String("foo".into()),
            ControlValueSetter::Bytes(vec![1, 2, 3]),
            ControlValueSetter::KeyValue(1, 2),
            ControlValueSetter::Point(0.0, 0.0),
            ControlValueSetter::RGB(0.0, 0.0, 0.0),
            ControlValueSetter::None,
            ControlValueSetter::EnumValue(7),
        ];
        for setter in &unsupported_setters {
            let err = v4l2_cid_value(cid, setter).unwrap_err();
            match err {
                NokhwaError::SetPropertyError {
                    property,
                    value,
                    error,
                } => {
                    assert_eq!(property, cid, "wrong property for {setter:?}");
                    assert_eq!(
                        value,
                        setter.to_string(),
                        "value field must round-trip the setter's Display"
                    );
                    assert_eq!(
                        error, "unsupported ControlValueSetter variant for V4L2 CID",
                        "canonical error string drifted for {setter:?}"
                    );
                },
                other => panic!("expected SetPropertyError for {setter:?}, got {other:?}"),
            }
        }
    }

    /// `set_live_property` (`brightness` / `contrast` / `hue` /
    /// `saturation` on `v4l2src`) has four branches; the two error
    /// branches return before `source.set_property` is reached and
    /// are therefore pure functions of the input.
    ///
    /// Branch 1 — `Integer(i)` with `i` outside `i32` range. The
    /// sibling `v4l2_cid_value` is `i64`-passthrough (the overflow
    /// check happens later inside `build_extra_controls`, which now
    /// returns `Err` — pinned by `build_extra_controls_errors_on_i64_outside_i32_range`).
    /// `set_live_property` instead errors *eagerly* — pin that
    /// divergence so a future refactor that unifies the two doesn't
    /// quietly drop user values.
    /// Pin all three fields of the resulting `SetPropertyError`
    /// (property name, the raw i64 stringified into `value`, canonical
    /// error string) verbatim. The `value` field on this branch is
    /// `i.to_string()` (raw integer text, e.g. `"2147483648"`) — *not*
    /// the setter's `Display` form (`"IntegerValue: 2147483648"`),
    /// which is what the unsupported-setter branch uses below. The
    /// asymmetry is deliberate (the unsupported-setter branch has no
    /// `i64` to peel out) and easy to flip during refactoring; pin it
    /// so a future "unify the two error shapes" change is forced to
    /// update this test.
    #[test]
    fn set_live_property_rejects_i64_outside_i32_range() {
        ensure_gst_init();
        let source = gstreamer::ElementFactory::make("fakesrc")
            .build()
            .expect("fakesrc must build for the test");
        let too_big_i64 = i64::from(i32::MAX) + 1;
        let err = set_live_property(
            &source,
            "brightness",
            &ControlValueSetter::Integer(too_big_i64),
        )
        .unwrap_err();
        match err {
            NokhwaError::SetPropertyError {
                property,
                value,
                error,
            } => {
                assert_eq!(property, "brightness");
                assert_eq!(value, too_big_i64.to_string());
                assert_eq!(
                    error,
                    "i64 value exceeds i32 range expected by v4l2src property"
                );
            },
            other => panic!("expected SetPropertyError, got {other:?}"),
        }

        let too_small_i64 = i64::from(i32::MIN) - 1;
        let err = set_live_property(
            &source,
            "brightness",
            &ControlValueSetter::Integer(too_small_i64),
        )
        .unwrap_err();
        match err {
            NokhwaError::SetPropertyError {
                property, value, ..
            } => {
                assert_eq!(property, "brightness");
                assert_eq!(value, too_small_i64.to_string());
            },
            other => panic!("expected SetPropertyError, got {other:?}"),
        }
    }

    /// Branch 2 — non-`Integer`/non-`Boolean` `ControlValueSetter`
    /// variants. The mirror of `v4l2_cid_value_rejects_unsupported_setters`,
    /// but for the live-property path: live properties are also
    /// `i32`-valued, so `Float` / `String` / `Bytes` / etc. cannot be
    /// represented and must produce a `SetPropertyError` with the
    /// canonical message. A regression that swapped the variant for
    /// `GeneralError`, dropped the rejected setter from the `value`
    /// field, or rephrased the canonical string would slip past every
    /// existing pin in this module — none of the live-property error
    /// paths are exercised today.
    #[test]
    fn set_live_property_rejects_unsupported_setters() {
        ensure_gst_init();
        let source = gstreamer::ElementFactory::make("fakesrc")
            .build()
            .expect("fakesrc must build for the test");
        let property = "brightness";
        let unsupported_setters = [
            ControlValueSetter::Float(1.5),
            ControlValueSetter::String("foo".into()),
            ControlValueSetter::Bytes(vec![1, 2, 3]),
            ControlValueSetter::KeyValue(1, 2),
            ControlValueSetter::Point(0.0, 0.0),
            ControlValueSetter::RGB(0.0, 0.0, 0.0),
            ControlValueSetter::None,
            ControlValueSetter::EnumValue(7),
        ];
        for setter in &unsupported_setters {
            let err = set_live_property(&source, property, setter).unwrap_err();
            match err {
                NokhwaError::SetPropertyError {
                    property: p,
                    value,
                    error,
                } => {
                    assert_eq!(p, property, "wrong property for {setter:?}");
                    assert_eq!(
                        value,
                        setter.to_string(),
                        "value field must round-trip the setter's Display"
                    );
                    assert_eq!(
                        error, "unsupported ControlValueSetter variant for live property",
                        "canonical error string drifted for {setter:?}"
                    );
                },
                other => panic!("expected SetPropertyError for {setter:?}, got {other:?}"),
            }
        }
    }

    /// Branch 3 — `Boolean(true) → 1`, `Boolean(false) → 0` mapping.
    /// Live properties are `i32`-valued, and v4l2src's documented
    /// behavior for boolean-shaped controls is the standard 0/1
    /// encoding. Pin so a future refactor that flipped the mapping
    /// (or replaced the explicit `i32::from(*b)` with a sign-extended
    /// `*b as i32` from a different bool representation) doesn't
    /// silently invert every boolean control write. Uses an element
    /// with a writable `bool` property (`fakesrc::is-live`) so we can
    /// actually observe the post-set value rather than relying on the
    /// no-op of writing to a missing property.
    #[test]
    fn set_live_property_boolean_maps_to_zero_or_one() {
        ensure_gst_init();
        let source = gstreamer::ElementFactory::make("fakesrc")
            .build()
            .expect("fakesrc must build for the test");
        // `fakesrc` doesn't have a `brightness` property, so we exploit
        // the fact that the i32 boolean encoding is fully determined
        // by the input alone — verify no panic + Ok by writing into a
        // property that does accept i32. `fakesrc` has `num-buffers`
        // (i32). The mapping `false → 0`, `true → 1` is what we pin;
        // the property write succeeds only when the encoding matches.
        set_live_property(&source, "num-buffers", &ControlValueSetter::Boolean(false))
            .expect("Boolean(false) must encode to a valid i32");
        let observed: i32 = source.property("num-buffers");
        assert_eq!(observed, 0, "Boolean(false) must map to i32 0");

        set_live_property(&source, "num-buffers", &ControlValueSetter::Boolean(true))
            .expect("Boolean(true) must encode to a valid i32");
        let observed: i32 = source.property("num-buffers");
        assert_eq!(observed, 1, "Boolean(true) must map to i32 1");
    }

    /// Pin: `list_controls` returns an empty list for an element whose
    /// properties contain none of the four v4l2src names
    /// (`brightness`/`contrast`/`hue`/`saturation`). This is the common
    /// case on Windows (`mfvideosrc`/`ksvideosrc`) and macOS
    /// (`avfvideosrc`); the module docstring promises an empty list
    /// there. `fakesrc` is the closest stock proxy: it has properties
    /// (`is-live`, `num-buffers`, `sync`, …) but none of them match.
    #[test]
    fn list_controls_returns_empty_for_element_without_matching_property_names() {
        ensure_gst_init();
        let source = gstreamer::ElementFactory::make("fakesrc")
            .build()
            .expect("fakesrc must build for the test");
        let controls = list_controls(&source);
        assert!(
            controls.is_empty(),
            "fakesrc has no v4l2src control names, list_controls must return empty; got {controls:?}"
        );
    }

    /// Pin: `list_controls` filters by exact property name even when
    /// the element exposes a much richer property surface than
    /// `fakesrc`. `videotestsrc` has 25+ properties (`pattern`, `kt`,
    /// `kx`, `motion`, `is-live`, …) but none match the four allow-listed
    /// v4l2src names. If the filter ever drifts to substring or
    /// case-insensitive matching this test will catch it.
    #[test]
    fn list_controls_returns_empty_for_videotestsrc() {
        ensure_gst_init();
        let source = gstreamer::ElementFactory::make("videotestsrc")
            .build()
            .expect("videotestsrc must build for the test");
        let controls = list_controls(&source);
        assert!(
            controls.is_empty(),
            "videotestsrc has no v4l2src control names, list_controls must return empty; got {controls:?}"
        );
    }
}
