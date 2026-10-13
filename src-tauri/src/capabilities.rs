//! Temporary product gates are enforced at the IPC and lifecycle boundaries.
pub const PROGRAM_PROXY: bool = false;
pub const ENHANCED_STABILITY: bool = false;
pub const CODEX_ATTACH: bool = false;

pub fn require_program_proxy() -> Result<(), crate::error::AppErrorDto> {
    if PROGRAM_PROXY {
        Ok(())
    } else {
        Err(
            crate::error::AppError::Conflict("程序代理暂时停用；已保存的应用条目保持不变".into())
                .dto(),
        )
    }
}
