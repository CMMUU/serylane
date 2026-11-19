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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Observation {
    Unknown,
    Absent,
    Present {
        failure: Option<Failure>,
        stale_binding: bool,
    },
}

fn probe() -> &'static SharedProbe<Observation> {
    static PROBE: OnceLock<SharedProbe<Observation>> = OnceLock::new();
    PROBE.get_or_init(SharedProbe::default)
}

pub(super) fn reset() {
    probe().reset();
}

pub(super) fn failure() -> Option<Failure> {
    match probe().read(Duration::from_millis(850), Duration::from_secs(5), inspect) {
        ProbeRead::Complete(Ok(Observation::Present { failure, .. })) => failure,
        // A failed diagnostic command proves nothing about service readiness.
        _ => None,
    }
}

/// Recheck immediately before detaching. A cached diagnosis may describe an
/// old job that launchd has already replaced or started.
pub(super) fn stale_binding_now() -> bool {
    matches!(
        inspect(),
        Ok(Observation::Present {
            failure: Some(Failure::SpawnFailed),
            stale_binding: true
        })
    )
}

pub(super) fn confirms_absence() -> bool {
    matches!(inspect(), Ok(Observation::Absent))
}

fn inspect() -> Result<Observation, String> {
    // A private temporary file avoids a full pipe deadlock. Only a bounded
    // excerpt of this fixed, system-owned command is read, and nothing persists.
    let mut output = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut errors = tempfile::tempfile().map_err(|error| error.to_string())?;
    let mut child = Command::new("/bin/launchctl")
        .args(["print", &format!("system/{}", super::protocol::LABEL)])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|error| error.to_string())?)
        .stderr(errors.try_clone().map_err(|error| error.to_string())?)
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_millis(750);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(Observation::Unknown);
            }
        }
    };
    if status.success() {
        Ok(
            read_bounded(&mut output)?.map_or(Observation::Unknown, |text| {
                parse(&text, env!("CARGO_PKG_VERSION"))
            }),
        )
    } else if status.code() == Some(113)
        && read_bounded(&mut errors)?.is_some_and(|text| reports_service_absent(&text))
    {
        Ok(Observation::Absent)
    } else {
        Ok(Observation::Unknown)
    }
}

fn read_bounded(file: &mut std::fs::File) -> Result<Option<String>, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok((bytes.len() <= 65_536).then(|| String::from_utf8_lossy(&bytes).into_owned()))
}

fn reports_service_absent(text: &str) -> bool {
    let expected = format!(
        "Could not find service \"{}\" in domain for system",
        super::protocol::LABEL
    );
    text.lines().any(|line| line.trim() == expected)
}

fn parse(text: &str, current_version: &str) -> Observation {
    let expected_job = format!("system/{} = {{", super::protocol::LABEL);
    if text.lines().next() != Some(expected_job.as_str()) {
        return Observation::Unknown;
    }
    let value = |key: &str| {
        text.lines().find_map(|line| {
            // Nested dictionaries can also contain keys named state or pid.
            let (name, value) = line.strip_prefix('\t')?.split_once(" = ")?;
            (name == key).then_some(value)
        })
    };
    // Historical exit codes remain present after a later successful launch.
    // Do not diagnose a currently running service using its old exit code.
    let failed = matches!(value("state"), Some("spawn scheduled" | "not running"))
        && value("pid").is_none()
        && value("active count") == Some("0")
        && value("job state") == Some("spawn failed")
        && value("last exit code").and_then(|code| code.split(':').next().map(str::trim))
            == Some("78");
    let program = value("program identifier")
        .and_then(|program| program.split_once(" (mode:").map(|(path, _)| path));
    let expected_program = format!("Contents/MacOS/{}", super::protocol::HELPER_BINARY_NAME);
    let reports_our_binding = value("managed_by") == Some("com.apple.xpc.ServiceManagement")
        && value("parent bundle identifier") == Some("com.cmmuu.mihomodesktop")
        && program == Some(expected_program.as_str());
    let old_version = value("parent bundle version")
        .and_then(version_triplet)
        .zip(version_triplet(current_version))
        .is_some_and(|(registered, current)| registered < current);
    // These observations are used solely to decide whether to ask
    // SMAppService to redo an existing association. They grant no trust.
    // A newly registered (same-version) job must not restart the transaction
    // even if it subsequently reports a launch constraint problem.
    Observation::Present {
        failure: failed.then_some(Failure::SpawnFailed),
        stale_binding: failed && reports_our_binding && old_version,
    }
}

fn version_triplet(text: &str) -> Option<[u32; 3]> {
    let mut parts = text.split('.');
    let version = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(fields: &str) -> String {
        format!(
            "system/{} = {{\n{fields}\n}}",
            super::super::protocol::LABEL
        )
    }

    #[test]
    fn observed_macos_spawn_failure_requires_reassociation_not_more_authorization() {
        assert_eq!(
            parse(&job("\tstate = spawn scheduled\n\tactive count = 0\n\tlast exit code = 78: EX_CONFIG\n\tjob state = spawn failed"), "0.7.44"),
            Observation::Present { failure: Some(Failure::SpawnFailed), stale_binding: false }
        );
    }

    #[test]
    fn unknown_partial_and_historical_output_never_asserts_failure() {
        assert_eq!(parse("", "0.7.44"), Observation::Unknown);
        assert_eq!(parse("system/other = {\n}", "0.7.44"), Observation::Unknown);
        for text in [
            "\tlast exit code = 78: EX_CONFIG",
            "\tstate = running\n\tlast exit code = 78: EX_CONFIG\n\tjob state = spawn failed",
            "\tjob state = spawn failed\n\tlast exit code = 178",
            "\tjob state = spawn failed\n\tlast exit code = 1",
            "\tlaunch info = {\n\t\tstate = spawn scheduled\n\t\tactive count = 0\n\t}\n\tlast exit code = 78: EX_CONFIG\n\tjob state = spawn failed",
        ] {
            assert_eq!(parse(&job(text), "0.7.44"), Observation::Present { failure: None, stale_binding: false }, "{text}");
        }
    }

    #[test]
    fn only_our_confirmed_older_failed_job_allows_automatic_reassociation() {
        let fixture = job(
            "\tactive count = 0\n\tmanaged_by = com.apple.xpc.ServiceManagement\n\tstate = spawn scheduled\n\tprogram identifier = Contents/MacOS/mihomo-tun-helper (mode: 2)\n\tparent bundle identifier = com.cmmuu.mihomodesktop\n\tparent bundle version = 0.7.42\n\tlast exit code = 78: EX_CONFIG\n\tjob state = spawn failed\n\tproperties = partial import | resolve program | needs LWCR update | has LWCR",
        );
        let eligible = |text: &str, ver| {
            matches!(
                parse(text, ver),
                Observation::Present {
                    stale_binding: true,
                    ..
                }
            )
        };
        assert!(eligible(&fixture, "0.7.44"));
        assert!(!eligible(&fixture, "0.7.42"));
        assert!(!eligible(&fixture, "0.7.41"));
        for unrelated in [
            fixture.replace("spawn scheduled", "running"),
            fixture.replace("active count = 0", "active count = 1"),
            fixture.replace("active count = 0", "active count = 0\n\tpid = 1234"),
            fixture.replace("parent bundle version = 0.7.42", "launch count = 500"),
            fixture.replace(
                "parent bundle identifier = com.cmmuu.mihomodesktop",
                "parent bundle identifier = com.example.another",
            ),
            fixture.replace("Contents/MacOS/mihomo-tun-helper", "Contents/MacOS/other"),
            fixture.replace(
                "managed_by = com.apple.xpc.ServiceManagement",
                "managed_by = different",
            ),
            fixture.replace("last exit code = 78", "last exit code = 1"),
        ] {
            assert!(!eligible(&unrelated, "0.7.44"), "{unrelated}");
        }
    }

    #[test]
    fn only_the_exact_system_job_not_found_diagnostic_counts_as_absence() {
        assert!(reports_service_absent(&format!(
            "Bad request.\nCould not find service \"{}\" in domain for system",
            super::super::protocol::LABEL,
        )));
        for unknown in [
            "",
            "Could not find specified service",
            "Operation not permitted",
            "Could not find service \"com.example.other\" in domain for system",
        ] {
            assert!(!reports_service_absent(unknown));
        }
    }
}
