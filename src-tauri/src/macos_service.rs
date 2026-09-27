//! SMAppService operations shared by login and privileged helpers. Call mutations
//! from a blocking worker, never the UI thread. Completion is not mere status.
use block2::RcBlock;
use objc2::{
    msg_send,
    runtime::{AnyClass, AnyObject},
};
use std::ffi::{c_char, CStr};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

pub const NOT_REGISTERED: isize = 0;
pub const ENABLED: isize = 1;
pub const REQUIRES_APPROVAL: isize = 2;
pub const NOT_FOUND: isize = 3;

pub struct Service {
    pub name: &'static CStr,
    pub agent: bool,
    // Remains set after a caller timeout until the OS completion actually runs.
    pending: AtomicBool,
}

impl Service {
    pub const fn new(name: &'static CStr, agent: bool) -> Self {
        Self {
            name,
            agent,
            pending: AtomicBool::new(false),
        }
    }

    unsafe fn object(&self) -> Result<*mut AnyObject, String> {
        let class = AnyClass::get(c"SMAppService").ok_or("此功能需要 macOS 13 或更新版本")?;
        let string = AnyClass::get(c"NSString").ok_or("系统字符串服务不可用")?;
        let name: *mut AnyObject = msg_send![string, stringWithUTF8String: self.name.as_ptr()];
        let service: *mut AnyObject = if self.agent {
            msg_send![class, agentServiceWithPlistName: name]
        } else {
            msg_send![class, daemonServiceWithPlistName: name]
        };
        if service.is_null() {
            Err("读取系统后台服务失败".into())
        } else {
            Ok(service)
        }
    }

    pub fn status(&self) -> Result<isize, String> {
        objc2::rc::autoreleasepool(|_| unsafe { Ok(msg_send![self.object()?, status]) })
    }

    pub fn is_unregistering(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    pub fn ensure_settled(&self) -> Result<(), String> {
        if self.is_unregistering() {
            Err("系统仍在退出旧服务，请稍后核对状态再重试；未重复登记".into())
        } else {
            Ok(())
        }
    }

    pub fn register(&self) -> Result<(), String> {
        self.ensure_settled()?;
        objc2::rc::autoreleasepool(|_| unsafe {
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let ok: bool = msg_send![self.object()?, registerAndReturnError: &mut error];
            if ok {
                Ok(())
            } else {
                Err(error_message(error))
            }
        })
    }

    pub fn unregister(&'static self) -> Result<(), String> {
        if self.pending.swap(true, Ordering::AcqRel) {
            return Err("旧服务退出尚未完成，请稍后重试；未重复执行".into());
        }
        objc2::rc::autoreleasepool(|_| unsafe {
            let service = match self.object() {
                Ok(service) => service,
                Err(error) => {
                    self.pending.store(false, Ordering::Release);
                    return Err(error);
                }
            };
            let raw: isize = msg_send![service, status];
            if raw == NOT_REGISTERED || raw == NOT_FOUND {
                self.pending.store(false, Ordering::Release);
                return Ok(());
            }
            let (sender, receiver) = mpsc::sync_channel(1);
            let completion = RcBlock::new(move |error: *mut AnyObject| {
                let result = if error.is_null() {
                    Ok(())
                } else {
                    Err(error_message(error))
                };
                self.pending.store(false, Ordering::Release);
                let _ = sender.send(result);
            });
            let _: () = msg_send![service, unregisterWithCompletionHandler: &*completion];
            receiver
                .recv_timeout(Duration::from_secs(15))
                .map_err(|_| {
                    "系统尚未确认旧服务退出，修复已暂停；请稍后核对状态，未注册第二个服务"
                        .to_string()
                })??;
            // Apple documents completion as the re-registration boundary. A
            // separate scheduling turn also avoids its reported immediate
            // re-registration race (Apple DTS thread 783539). This bounded
            // delay runs only after successful removal, never on the UI thread.
            std::thread::sleep(Duration::from_secs(2));
            Ok(())
        })
    }
}

pub fn open_settings() -> Result<(), String> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let class = AnyClass::get(c"SMAppService").ok_or("此功能需要 macOS 13 或更新版本")?;
        let _: () = msg_send![class, openSystemSettingsLoginItems];
        Ok(())
    })
}

unsafe fn error_message(error: *mut AnyObject) -> String {
    if error.is_null() {
        return "系统服务操作未完成，请核对登录项与扩展设置".into();
    }
    let code: isize = msg_send![error, code];
    let description: *mut AnyObject = msg_send![error, localizedDescription];
    if description.is_null() {
        return format!("系统服务操作未完成（代码 {code}）");
    }
    let utf8: *const c_char = msg_send![description, UTF8String];
    if utf8.is_null() {
        return format!("系统服务操作未完成（代码 {code}）");
    }
    format!("{}（代码 {code}）", CStr::from_ptr(utf8).to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_exit_blocks_registration_before_any_system_call() {
        let service = Service::new(c"fixture.plist", true);
        assert!(service.ensure_settled().is_ok());
        service.pending.store(true, Ordering::Release);
        assert!(service.is_unregistering());
        assert!(service.ensure_settled().is_err());
        assert!(service.register().is_err());
        service.pending.store(false, Ordering::Release);
        assert!(service.ensure_settled().is_ok());
    }
}
