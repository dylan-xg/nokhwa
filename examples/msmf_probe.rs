//! Ad-hoc MSMF enumeration probe. Prints every device that
//! `nokhwa::query(ApiBackend::MediaFoundation)` sees.

#[cfg(all(feature = "input-msmf", target_os = "windows"))]
fn main() {
    use nokhwa::query;
    use nokhwa::utils::ApiBackend;
    match query(ApiBackend::MediaFoundation) {
        Ok(cams) => {
            println!("MSMF enumeration: {} device(s)", cams.len());
            for c in &cams {
                println!(
                    "  - {} | desc='{}' | misc='{}'",
                    c.human_name(),
                    c.description(),
                    c.misc()
                );
            }
        },
        Err(e) => println!("error: {e}"),
    }
}

#[cfg(not(all(feature = "input-msmf", target_os = "windows")))]
fn main() {
    eprintln!("input-msmf + target_os=windows required");
}
