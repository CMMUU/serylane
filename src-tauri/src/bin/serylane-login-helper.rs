//! Short-lived login launcher. The registered process is deliberately not the
//! UI process: re-registering an upgraded agent must not terminate Serylane.
#[cfg(target_os = "macos")]
fn main() {
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let executable = std::env::current_exe()?;
        let bundle = executable
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
            .ok_or("login launcher is not inside an application bundle")?;
        // Exact containing bundle, not a bundle-ID lookup that could launch a
        // different installed copy. No shell interpolation or proxy changes.
        let status = std::process::Command::new("/usr/bin/open")
            .arg("-g")
            .arg("-a")
            .arg(bundle)
            .arg("--args")
            .arg("--autostart")
            .status()?;
        if !status.success() {
            return Err("LaunchServices did not open Serylane".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Serylane login launch: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Serylane native login launcher is only used on macOS");
}
