//! Read-only identity probe for native installation acceptance. It exits before
//! Tauri, login helpers, user storage, single-instance handling or proxy startup.
pub fn update_probe() -> bool {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 2 || args[1] != "--serylane-update-probe" {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        let verified = crate::tun_service::codesign::bundle_root().and_then(|root| {
            crate::tun_service::codesign::validate_bundle(&root, env!("CARGO_PKG_VERSION"))
        });
        if let Err(error) = verified {
            eprintln!("{}", crate::app_log::sanitize(&error));
            std::process::exit(1);
        }
    }
    println!(
        "{}",
        serde_json::json!({"version":env!("CARGO_PKG_VERSION"),
        "identifier":"com.cmmuu.mihomodesktop", "os":std::env::consts::OS, "arch":std::env::consts::ARCH})
    );
    true
}
