use std::ffi::{c_void, CStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

type CfRef = *const c_void;
type CfUrlRef = *const c_void;
type CfStringRef = *const c_void;
type SecStaticCodeRef = *mut c_void;
type SecRequirementRef = *mut c_void;
type OsStatus = i32;

const ERR_SUCCESS: OsStatus = 0;
const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(allocator: CfRef, text: *const i8, encoding: u32) -> CfStringRef;
    fn CFDictionaryGetValue(dictionary: CfRef, key: CfRef) -> CfRef;
    fn CFGetTypeID(value: CfRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CfRef,
        buffer: *const u8,
        buffer_length: isize,
        is_directory: bool,
    ) -> CfUrlRef;
    fn CFStringGetLength(value: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut i8,
        buffer_size: isize,
        encoding: u32,
    ) -> bool;
    fn CFRelease(value: CfRef);
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecCodeInfoTeamIdentifier: CfStringRef;
    fn SecCodeCopySigningInformation(
        code: SecStaticCodeRef,
        flags: u32,
        info: *mut CfRef,
    ) -> OsStatus;
    fn SecRequirementCreateWithString(
        text: CfStringRef,
        flags: u32,
        requirement: *mut SecRequirementRef,
    ) -> OsStatus;
    fn SecStaticCodeCreateWithPath(
        path: CfUrlRef,
        flags: u32,
        code: *mut SecStaticCodeRef,
    ) -> OsStatus;
    fn SecStaticCodeCheckValidity(
        code: SecStaticCodeRef,
        flags: u32,
        requirement: SecRequirementRef,
    ) -> OsStatus;
    fn SecCodeCopyDesignatedRequirement(
        code: SecStaticCodeRef,
        flags: u32,
        requirement: *mut SecRequirementRef,
    ) -> OsStatus;
    fn SecRequirementCopyString(
        requirement: SecRequirementRef,
        flags: u32,
        text: *mut CfStringRef,
    ) -> OsStatus;
}

struct OwnedCf(CfRef);

impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

pub fn sibling_executable(name: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = current
        .parent()
        .ok_or_else(|| "当前可执行文件没有父目录".to_string())?;
    Ok(directory.join(name))
}

pub fn bundle_root() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let macos = current
        .parent()
        .ok_or_else(|| "当前可执行文件不在 app bundle 中".to_string())?;
    let contents = macos
        .parent()
        .ok_or_else(|| "当前可执行文件不在 Contents/MacOS 中".to_string())?;
    let bundle = contents
        .parent()
        .ok_or_else(|| "当前可执行文件不在 app bundle 中".to_string())?;
    if bundle.extension().and_then(|value| value.to_str()) != Some("app") {
        return Err("当前构建不是 macOS app bundle".to_string());
    }
    Ok(bundle.to_path_buf())
}

pub fn bundle_layout_ready() -> bool {
    let Ok(bundle) = bundle_root() else {
        return false;
    };
    let plist = bundle
        .join("Contents/Library/LaunchDaemons")
        .join(super::protocol::PLIST_NAME.to_string_lossy().as_ref());
    let helper = bundle
        .join("Contents/MacOS")
        .join(super::protocol::HELPER_BINARY_NAME);
    plist.is_file() && helper.is_file()
}

pub fn designated_requirement(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("拒绝非普通签名文件：{}", path.display()));
    }
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    let bytes = canonical.as_os_str().as_bytes();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            false,
        );
        if url.is_null() {
            return Err("创建签名校验 URL 失败".to_string());
        }
        let _url = OwnedCf(url);
        let mut code: SecStaticCodeRef = ptr::null_mut();
        let status = SecStaticCodeCreateWithPath(url, 0, &mut code);
        if status != ERR_SUCCESS || code.is_null() {
            return Err(format!("读取代码签名失败：OSStatus {status}"));
        }
        let _code = OwnedCf(code.cast());
        let status = SecStaticCodeCheckValidity(code, 0, ptr::null_mut());
        if status != ERR_SUCCESS {
            return Err(format!("代码签名无效：OSStatus {status}"));
        }
        let mut requirement: SecRequirementRef = ptr::null_mut();
        let status = SecCodeCopyDesignatedRequirement(code, 0, &mut requirement);
        if status != ERR_SUCCESS || requirement.is_null() {
            return Err(format!("读取指定要求失败：OSStatus {status}"));
        }
        let _requirement = OwnedCf(requirement.cast());
        let mut text: CfStringRef = ptr::null();
        let status = SecRequirementCopyString(requirement, 0, &mut text);
        if status != ERR_SUCCESS || text.is_null() {
            return Err(format!("转换指定要求失败：OSStatus {status}"));
        }
        let _text = OwnedCf(text);
        cf_string(text)
    }
}

pub fn validate(path: &Path) -> Result<(), String> {
    designated_requirement(path).map(|_| ())
}

/// Integrity alone accepts ad-hoc signatures; a privileged service also needs
/// a stable Developer ID identity shared by the application and its executables.
/// This deliberately does not claim notarization or successful launchd execution.
pub fn validate_installation() -> Result<(), String> {
    let bundle = bundle_root()?;
    if !bundle_layout_ready() {
        return Err("应用包缺少 TUN 辅助程序或服务清单，请从下载中心安装完整版本".into());
    }
    let team = developer_team(&bundle)?;
    for name in [
        super::protocol::HELPER_BINARY_NAME,
        super::protocol::CORE_BINARY_NAME,
    ] {
        let actual = developer_team(&sibling_executable(name)?)?;
        if actual != team {
            return Err("主应用与辅助程序的签名团队不一致，请从下载中心重新安装完整版本".into());
        }
    }
    Ok(())
}

fn developer_team(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err("安装包签名对象不是普通文件或应用包，请重新安装".into());
    }
    let bytes = path.as_os_str().as_bytes();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            metadata.is_dir(),
        );
        if url.is_null() {
            return Err("读取安装包签名地址失败".into());
        }
        let _url = OwnedCf(url);
        let mut code = ptr::null_mut();
        let status = SecStaticCodeCreateWithPath(url, 0, &mut code);
        if status != ERR_SUCCESS || code.is_null() {
            return Err(format!(
                "读取安装包代码签名失败（OSStatus {status}），请安装完整版本"
            ));
        }
        let _code = OwnedCf(code.cast());
        let text = CFStringCreateWithCString(
            ptr::null(),
            c"anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
                .as_ptr(),
            UTF8,
        );
        if text.is_null() {
            return Err("创建开发者签名校验失败".into());
        }
        let _text = OwnedCf(text);
        let mut requirement = ptr::null_mut();
        let status = SecRequirementCreateWithString(text, 0, &mut requirement);
        if status != ERR_SUCCESS || requirement.is_null() {
            return Err(format!("创建开发者签名要求失败（OSStatus {status}）"));
        }
        let _requirement = OwnedCf(requirement.cast());
        let status = SecStaticCodeCheckValidity(code, 1 << 4, requirement);
        if status != ERR_SUCCESS {
            return Err(format!("当前安装包未通过 Developer ID 签名检查，TUN 暂未就绪。请核对发行说明中的 macOS 签名状态，安装签名有效的完整包；临时签名包重复下载同一版本或重新授权不会补齐开发者身份（OSStatus {status}）"));
        }
        let mut info = ptr::null();
        let status = SecCodeCopySigningInformation(code, 1 << 1, &mut info);
        if status != ERR_SUCCESS || info.is_null() {
            return Err(format!("读取签名团队失败（OSStatus {status}）"));
        }
        let _info = OwnedCf(info);
        let team = CFDictionaryGetValue(info, kSecCodeInfoTeamIdentifier);
        if team.is_null() || CFGetTypeID(team) != CFStringGetTypeID() {
            return Err("安装包缺少签名团队，请重新安装正式版本".into());
        }
        let team = cf_string(team)?;
        if team.is_empty() {
            return Err("安装包签名团队为空，请重新安装正式版本".into());
        }
        Ok(team)
    }
}

unsafe fn cf_string(value: CfStringRef) -> Result<String, String> {
    let length = CFStringGetLength(value);
    let capacity = CFStringGetMaximumSizeForEncoding(length, UTF8) + 1;
    if capacity <= 1 {
        return Err("签名要求字符串为空".to_string());
    }
    let mut buffer = vec![0_i8; capacity as usize];
    if !CFStringGetCString(value, buffer.as_mut_ptr(), capacity, UTF8) {
        return Err("读取签名要求字符串失败".to_string());
    }
    Ok(CStr::from_ptr(buffer.as_ptr())
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    #[ignore = "requires an explicitly supplied Developer ID-signed test bundle"]
    fn signed_bundle_identity_smoke() {
        let bundle = PathBuf::from(
            std::env::var_os("SERYLANE_SIGNED_TEST_BUNDLE")
                .expect("supply an isolated signed bundle"),
        );
        let team = developer_team(&bundle).expect("Developer ID application identity");
        for name in [
            "serylane",
            "mihomo-tun-helper",
            "serylane-login-helper",
            "mihomo",
        ] {
            assert_eq!(
                developer_team(&bundle.join("Contents/MacOS").join(name)).unwrap(),
                team
            );
        }
    }

    #[test]
    fn valid_adhoc_integrity_is_not_a_developer_identity() {
        // Never inspect or modify the user's installed app in this regression.
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("fixture");
        std::fs::copy("/usr/bin/true", &binary).unwrap();
        assert!(Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&binary)
            .status()
            .unwrap()
            .success());
        assert!(validate(&binary).is_ok());
        assert!(developer_team(&binary)
            .unwrap_err()
            .contains("Developer ID"));
    }
}
