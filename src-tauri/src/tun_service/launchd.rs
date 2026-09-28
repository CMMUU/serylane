//! Read-only, bounded launchd diagnostics. Its human-readable output is never
//! used to grant readiness; only the authenticated XPC handshake can do that.
use super::lifecycle::{ProbeRead, SharedProbe};
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Failure {
    SpawnFailed,
}

fn probe() -> &'static SharedProbe<Option<Failure>> {
    static PROBE: OnceLock<SharedProbe<Option<Failure>>> = OnceLock::new();
    PROBE.get_or_init(SharedProbe::default)
}

pub(super) fn reset() {
    probe().reset();
}

pub(super) fn failure() -> Option<Failure> {
    match probe().read(Duration::from_millis(850), Duration::from_secs(5), inspect) {
        ProbeRead::Complete(Ok(value)) => value,
        // A failed diagnostic command proves nothing about service readiness.
        _ => None,
    }
}

fn inspect() -> Result<Option<Failure>, String> {
    // A private temporary file avoids a full pipe deadlock. Only a bounded
    // excerpt of this fixed, system-owned command is read, and nothing persists.
    let mut output = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut child = Command::new("/bin/launchctl")
        .args(["print", &format!("system/{}", super::protocol::LABEL)])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|error| error.to_string())?)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(750);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Ok(None);
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    }
    output
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    output
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65_536 {
        return Ok(None);
    }
    Ok(parse(&String::from_utf8_lossy(&bytes)))
}

fn parse(text: &str) -> Option<Failure> {
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(" = ")?;
            (name == key).then_some(value)
        })
    };
    // Historical exit codes remain present after a later successful launch.
    // Do not diagnose a currently running service using its old exit code.
    if value("state") == Some("running") || value("job state") != Some("spawn failed") {
        return None;
    }
    let exit = value("last exit code")?.split(':').next()?.trim();
    (exit == "78").then_some(Failure::SpawnFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_macos_spawn_failure_requires_reassociation_not_more_authorization() {
        assert_eq!(parse("state = spawn scheduled\nlast exit code = 78: EX_CONFIG\njob state = spawn failed\n"), Some(Failure::SpawnFailed));
    }

    #[test]
    fn unknown_partial_and_historical_output_never_asserts_failure() {
        for text in [
            "",
            "last exit code = 78: EX_CONFIG",
            "state = running\nlast exit code = 78: EX_CONFIG\njob state = spawn failed",
            "job state = spawn failed\nlast exit code = 178",
            "job state = spawn failed\nlast exit code = 1",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
