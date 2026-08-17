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

use nokhwa_core::{
    error::NokhwaError,
    types::{ApiBackend, CameraInfo},
};

/// Gets the native [`ApiBackend`]
#[must_use]
pub fn native_api_backend() -> Option<ApiBackend> {
    match std::env::consts::OS {
        "linux" => Some(ApiBackend::Video4Linux),
        "macos" | "ios" => Some(ApiBackend::AVFoundation),
        "windows" => Some(ApiBackend::MediaFoundation),
        _ => None,
    }
}

/// Query the system for a list of available devices. Please refer to the API Backends that support `Query`) <br>
/// Usually the order goes Native -> `GStreamer`.
/// # Quirks
/// - `Media Foundation`: The symbolic link for the device is listed in the `misc` attribute of the [`CameraInfo`].
/// - `Media Foundation`: The names may contain invalid characters since they were converted from UTF16.
/// - `AVFoundation`: The ID of the device is stored in the `misc` attribute of the [`CameraInfo`].
/// - `AVFoundation`: There is lots of miscellaneous info in the `desc` attribute.
/// - `WASM`: The `misc` field contains the device ID and group ID are seperated by a space (' ')
/// # Errors
/// If you use an unsupported API (check the README or crate root for more info), incompatible backend for current platform, incompatible platform, or insufficient permissions, etc
/// this will error.
pub fn query(api: ApiBackend) -> Result<Vec<CameraInfo>, NokhwaError> {
    match api {
        ApiBackend::Auto => {
            // determine platform
            match std::env::consts::OS {
                "linux" => {
                    if cfg!(feature = "input-v4l") && cfg!(target_os = "linux") {
                        query(ApiBackend::Video4Linux)
                    } else if cfg!(feature = "input-gstreamer") {
                        query(ApiBackend::GStreamer)
                    } else {
                        #[cfg(feature = "logging")]
                        log::warn!("No suitable backends available on Linux. Perhaps you meant to enable one of the backends such as `input-v4l`? (Please read the docs.)");
                        Err(NokhwaError::UnsupportedOperationError(ApiBackend::Auto))
                    }
                },
                "windows" => {
                    if cfg!(feature = "input-msmf") && cfg!(target_os = "windows") {
                        query(ApiBackend::MediaFoundation)
                    } else if cfg!(feature = "input-gstreamer") {
                        query(ApiBackend::GStreamer)
                    } else {
                        #[cfg(feature = "logging")]
                        log::warn!("No suitable backends available on Windows. Perhaps you meant to enable one of the backends such as `input-msmf`? (Please read the docs.)");
                        Err(NokhwaError::UnsupportedOperationError(ApiBackend::Auto))
                    }
                },
                "macos" => {
                    if cfg!(feature = "input-avfoundation") {
                        query(ApiBackend::AVFoundation)
                    } else if cfg!(feature = "input-gstreamer") {
                        query(ApiBackend::GStreamer)
                    } else {
                        #[cfg(feature = "logging")]
                        log::warn!("No suitable backends available on macOS. Perhaps you meant to enable one of the backends such as `input-avfoundation`? (Please read the docs.)");
                        Err(NokhwaError::UnsupportedOperationError(ApiBackend::Auto))
                    }
                },
                "ios" => {
                    if cfg!(feature = "input-avfoundation") {
                        query(ApiBackend::AVFoundation)
                    } else {
                        #[cfg(feature = "logging")]
                        log::warn!("No suitable backends available on iOS. Perhaps you meant to enable one of the backends such as `input-avfoundation`? (Please read the docs.)");
                        Err(NokhwaError::UnsupportedOperationError(ApiBackend::Auto))
                    }
                },
                _ => {
                    #[cfg(feature = "logging")]
                    log::warn!(
                        "No suitable backends available. You are on an unsupported platform."
                    );
                    Err(NokhwaError::NotImplementedError("Bad Platform".to_string()))
                },
            }
        },
        ApiBackend::AVFoundation => query_avfoundation(),
        ApiBackend::Video4Linux => query_v4l(),
        ApiBackend::MediaFoundation => query_msmf(),
        ApiBackend::GStreamer => query_gstreamer(),
        ApiBackend::Browser => query_wasm(),
        ApiBackend::Custom(_) => Err(NokhwaError::UnsupportedOperationError(api)),
    }
}

#[cfg(all(feature = "input-v4l", target_os = "linux"))]
fn query_v4l() -> Result<Vec<CameraInfo>, NokhwaError> {
    nokhwa_bindings_linux_v4l::query()
}

#[cfg(any(not(feature = "input-v4l"), not(target_os = "linux")))]
fn query_v4l() -> Result<Vec<CameraInfo>, NokhwaError> {
    Err(NokhwaError::UnsupportedOperationError(
        ApiBackend::Video4Linux,
    ))
}

// please refer to https://docs.microsoft.com/en-us/windows/win32/medfound/enumerating-video-capture-devices
#[cfg(all(feature = "input-msmf", target_os = "windows"))]
fn query_msmf() -> Result<Vec<CameraInfo>, NokhwaError> {
    nokhwa_bindings_windows_msmf::wmf::query()
}

#[cfg(feature = "input-gstreamer")]
fn query_gstreamer() -> Result<Vec<CameraInfo>, NokhwaError> {
    nokhwa_bindings_gstreamer::query()
}

#[cfg(not(feature = "input-gstreamer"))]
fn query_gstreamer() -> Result<Vec<CameraInfo>, NokhwaError> {
    Err(NokhwaError::UnsupportedOperationError(
        ApiBackend::GStreamer,
    ))
}

#[cfg(any(not(feature = "input-msmf"), not(target_os = "windows")))]
fn query_msmf() -> Result<Vec<CameraInfo>, NokhwaError> {
    Err(NokhwaError::UnsupportedOperationError(
        ApiBackend::MediaFoundation,
    ))
}

#[cfg(all(
    feature = "input-avfoundation",
    any(target_os = "macos", target_os = "ios")
))]
fn query_avfoundation() -> Result<Vec<CameraInfo>, NokhwaError> {
    nokhwa_bindings_macos_avfoundation::query()
}

#[cfg(not(all(
    feature = "input-avfoundation",
    any(target_os = "macos", target_os = "ios")
)))]
fn query_avfoundation() -> Result<Vec<CameraInfo>, NokhwaError> {
    Err(NokhwaError::UnsupportedOperationError(
        ApiBackend::AVFoundation,
    ))
}

fn query_wasm() -> Result<Vec<CameraInfo>, NokhwaError> {
    Err(NokhwaError::UnsupportedOperationError(ApiBackend::Browser))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_api_backend_matches_current_os() {
        let got = native_api_backend();
        if cfg!(target_os = "linux") {
            assert_eq!(got, Some(ApiBackend::Video4Linux));
        } else if cfg!(any(target_os = "macos", target_os = "ios")) {
            assert_eq!(got, Some(ApiBackend::AVFoundation));
        } else if cfg!(target_os = "windows") {
            assert_eq!(got, Some(ApiBackend::MediaFoundation));
        } else {
            assert_eq!(got, None);
        }
    }

    #[test]
    fn query_custom_is_unsupported() {
        let api = ApiBackend::Custom("my-backend".to_string());
        let result = query(api.clone());
        match result {
            Err(NokhwaError::UnsupportedOperationError(returned)) => {
                assert_eq!(returned, api);
            },
            other => panic!("expected UnsupportedOperationError(Custom), got {other:?}"),
        }
    }

    #[test]
    fn query_browser_is_unsupported() {
        match query(ApiBackend::Browser) {
            Err(NokhwaError::UnsupportedOperationError(ApiBackend::Browser)) => {},
            other => panic!("expected UnsupportedOperationError(Browser), got {other:?}"),
        }
    }

    // Each `query_*` stub returns the matching `UnsupportedOperationError` when
    // its backend is disabled. We can only assert this for the backends that
    // are actually compiled out in the current build configuration — running
    // these tests under different feature combos collectively covers every
    // disabled arm.

    #[cfg(any(not(feature = "input-v4l"), not(target_os = "linux")))]
    #[test]
    fn query_v4l_is_unsupported_when_disabled() {
        match query(ApiBackend::Video4Linux) {
            Err(NokhwaError::UnsupportedOperationError(ApiBackend::Video4Linux)) => {},
            other => panic!("expected UnsupportedOperationError(Video4Linux), got {other:?}"),
        }
    }

    #[cfg(any(not(feature = "input-msmf"), not(target_os = "windows")))]
    #[test]
    fn query_msmf_is_unsupported_when_disabled() {
        match query(ApiBackend::MediaFoundation) {
            Err(NokhwaError::UnsupportedOperationError(ApiBackend::MediaFoundation)) => {},
            other => panic!("expected UnsupportedOperationError(MediaFoundation), got {other:?}"),
        }
    }

    #[cfg(not(all(
        feature = "input-avfoundation",
        any(target_os = "macos", target_os = "ios")
    )))]
    #[test]
    fn query_avfoundation_is_unsupported_when_disabled() {
        match query(ApiBackend::AVFoundation) {
            Err(NokhwaError::UnsupportedOperationError(ApiBackend::AVFoundation)) => {},
            other => panic!("expected UnsupportedOperationError(AVFoundation), got {other:?}"),
        }
    }

    #[cfg(not(feature = "input-gstreamer"))]
    #[test]
    fn query_gstreamer_is_unsupported_when_disabled() {
        match query(ApiBackend::GStreamer) {
            Err(NokhwaError::UnsupportedOperationError(ApiBackend::GStreamer)) => {},
            other => panic!("expected UnsupportedOperationError(GStreamer), got {other:?}"),
        }
    }
}
