use super::{TunHelperState, TunHelperStatus};
use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) const SESSION_INSTRUCTIONS: &str =
    "Windows TUN 使用管理员会话模式；请从托盘退出应用后，右键选择“以管理员身份运行”。无需安装 TUN Helper 或 Windows 服务。";

// TokenElevation checks the effective process token, not administrator group
// membership: a filtered UAC token must still ask the user to restart elevated.
fn is_elevated() -> io::Result<bool> {
    let mut token = std::ptr::null_mut();
    // SAFETY: token is a writable out parameter and the pseudo process handle
    // remains valid for the lifetime of this process.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned a new owned handle on success.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: the buffer matches TOKEN_ELEVATION and remains live for the call.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn status_from_elevation(elevated: io::Result<bool>) -> TunHelperStatus {
    let (state, message, last_error) = match elevated {
        Ok(true) => (
            TunHelperState::Ready,
            "Windows TUN 管理员会话已就绪；内核随应用停止或退出，无需安装后台服务。".to_string(),
            None,
        ),
        Ok(false) => (
            TunHelperState::RequiresApproval,
            SESSION_INSTRUCTIONS.to_string(),
            None,
        ),
        Err(error) => {
            let message = format!("无法检查 Windows 管理员权限：{error}");
            (TunHelperState::Unreachable, message.clone(), Some(message))
        }
    };
    TunHelperStatus {
        supported: true,
        state,
        message,
        // This is a session capability check, not an installed helper or IPC
        // protocol. Runtime state comes from MihomoRuntime on Windows.
        protocol_version: 0,
        helper_version: None,
        runtime_running: false,
        runtime_pid: None,
        runtime_version: None,
        last_error,
    }
}

pub(super) fn status() -> TunHelperStatus {
    status_from_elevation(is_elevated())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_controls_session_readiness_without_claiming_a_service() {
        let normal = status_from_elevation(Ok(false));
        assert_eq!(normal.state, TunHelperState::RequiresApproval);
        assert!(normal.supported);
        assert!(!normal.ready());
        assert!(normal.message.contains("托盘退出"));
        let elevated = status_from_elevation(Ok(true));
        assert!(elevated.ready());
        assert_eq!(elevated.protocol_version, 0);
        assert!(!elevated.runtime_running);
        assert!(elevated.runtime_pid.is_none());
        let failed = status_from_elevation(Err(io::Error::from_raw_os_error(5)));
        assert_eq!(failed.state, TunHelperState::Unreachable);
        assert!(!failed.ready());
        assert!(failed.last_error.is_some());
    }

    #[test]
    fn reads_the_current_process_token_without_elevation() {
        assert!(is_elevated().is_ok());
    }
}
