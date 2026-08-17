/*
 * Copyright 2022 l1npengtul <l1npengtul@protonmail.com> / The Nokhwa Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 */

//! Minimal tokio example. Opens the default camera, pulls a handful of
//! frames asynchronously, then stops.
//!
//! Run with (on macOS native backend is selected automatically):
//!     cargo run -p nokhwa-tokio --example tokio_runner

use nokhwa::{open, OpenRequest, RunnerConfig};
use nokhwa_core::error::NokhwaError;
use nokhwa_core::types::CameraIndex;
use nokhwa_tokio::TokioCameraRunner;
use std::time::Duration;

fn main() -> Result<(), NokhwaError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| NokhwaError::general(format!("tokio runtime: {e}")))?;
    rt.block_on(async {
        let opened = open(CameraIndex::Index(0), OpenRequest::any())?;
        let mut runner = TokioCameraRunner::spawn(opened, RunnerConfig::default())?;

        if let Some(rx) = runner.frames_mut() {
            for _ in 0..5 {
                match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
                    Ok(Some(buf)) => println!("frame: {} bytes", buf.buffer().len()),
                    Ok(None) => {
                        eprintln!("runner stopped unexpectedly");
                        break;
                    },
                    Err(_) => {
                        eprintln!("frame timeout");
                        break;
                    },
                }
            }
        }

        runner.stop().await
    })
}
