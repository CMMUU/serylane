//! macOS application identity and LaunchServices adapter. A bundle is an atomic
//! application, not a directory of selectable helper executables. No operation
//! changes the system proxy, global environment, quarantine or code signature.
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::ptr;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSOpenPanel, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{
    NSArray, NSData, NSDate, NSDictionary, NSError, NSFileManager, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSString, NSURL,
};

use super::desktop::{DesktopApplication, DesktopBinding};
use super::AppAvailability;

const MAX_PLIST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const SCAN_BUDGET: Duration = Duration::from_secs(8);
const LAUNCH_WAIT: Duration = Duration::from_secs(15);

fn file_url(path: &Path) -> Result<Retained<NSURL>, String> {
    let path = path.to_str().ok_or("应用路径不是有效的 Unicode 路径")?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(path)))
}

fn is_app(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

fn plist(path: &Path) -> Result<Retained<NSDictionary>, String> {
    // NSBundle caches Info.plist by URL. Read fresh bytes instead so an update
    // at the same path is visible without restarting Serylane.
    let file = fs::File::open(path).map_err(|error| format!("读取应用信息失败：{error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_PLIST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PLIST_BYTES {
        return Err("应用信息文件超过读取上限".into());
    }
    let value = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(&bytes),
            NSPropertyListMutabilityOptions::Immutable,
            ptr::null_mut(),
        )
    }
    .map_err(|error| format!("应用信息格式异常：{}", error.localizedDescription()))?;
    value
        .downcast::<NSDictionary>()
        .map_err(|_| "应用信息必须是属性字典".into())
}

fn string(dict: &NSDictionary, key: &str) -> Option<String> {
    dict.objectForKey(&NSString::from_str(key))?
        .downcast_ref::<NSString>()
        .map(ToString::to_string)
        .filter(|s| !s.trim().is_empty())
}

fn bundle(path: &Path, validate: bool) -> Result<DesktopApplication, String> {
    if !path.is_absolute() || !is_app(path) {
        return Err("请选择完整的 .app 应用，而不是 Contents 内的文件".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("读取应用位置失败：{error}"))?;
    if !canonical.is_dir() {
        return Err("所选项目不是应用包".into());
    }
    let info = plist(&canonical.join("Contents/Info.plist"))?;
    let bundle_id =
        string(&info, "CFBundleIdentifier").ok_or("应用没有 Bundle ID，请选择另一应用")?;
    if bundle_id.len() > 255 || bundle_id.chars().any(char::is_control) {
        return Err("应用 Bundle ID 格式异常".into());
    }
    if string(&info, "CFBundlePackageType").is_some_and(|value| value != "APPL") {
        return Err("所选 bundle 不是可启动的桌面应用".into());
    }
    let entry = string(&info, "CFBundleExecutable").ok_or("应用缺少启动入口")?;
    if !matches!(
        Path::new(&entry)
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [Component::Normal(_)]
    ) {
        return Err("应用启动入口包含越界路径".into());
    }
    let executable = canonical.join("Contents/MacOS").join(&entry);
    let executable = executable
        .canonicalize()
        .map_err(|error| format!("应用启动文件暂时不可用：{error}"))?;
    if !executable.starts_with(&canonical) || !executable.is_file() {
        return Err("应用启动文件不在所选应用包内".into());
    }
    if fs::metadata(&executable)
        .map_err(|error| error.to_string())?
        .permissions()
        .mode()
        & 0o111
        == 0
    {
        return Err("应用启动文件没有执行权限".into());
    }
    let requirement = signature(&canonical, validate, None)?;
    let name = string(&info, "CFBundleDisplayName")
        .or_else(|| string(&info, "CFBundleName"))
        .unwrap_or_else(|| {
            NSFileManager::defaultManager()
                .displayNameAtPath(&NSString::from_str(&path.to_string_lossy()))
                .to_string()
        });
    let detail = if requirement.is_some() {
        "按应用身份关联；正常更新后继续使用当前安装版本"
    } else {
        "按所选安装位置关联；应用移动或签名身份变化后请重新关联"
    };
    Ok(DesktopApplication {
        binding: DesktopBinding::Macos {
            bundle_id,
            location: path.to_string_lossy().into_owned(),
            requirement,
        },
        name,
        version: string(&info, "CFBundleShortVersionString")
            .or_else(|| string(&info, "CFBundleVersion"))
            .unwrap_or_default(),
        root: canonical.to_string_lossy().into_owned(),
        executable: executable.to_string_lossy().into_owned(),
        availability: AppAvailability::Ready,
        detail: detail.into(),
        arguments: Vec::new(),
        working_directory: None,
    })
}

/// Inspect an explicitly selected application, including its code identity.
pub fn inspect(path: &Path) -> Result<DesktopApplication, String> {
    objc2::rc::autoreleasepool(|_| bundle(path, true))
}

/// The default search is bounded and does not descend into application bundles,
/// directory symlinks, network roots, backup volumes or user document folders.
pub fn collect() -> Result<Vec<DesktopApplication>, String> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    collect_roots(&roots)
}

fn collect_roots(roots: &[PathBuf]) -> Result<Vec<DesktopApplication>, String> {
    objc2::rc::autoreleasepool(|_| {
        let started = Instant::now();
        let mut todo: Vec<_> = roots.iter().map(|root| (root.clone(), 0)).collect();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        let mut visited = 0;
        while let Some((directory, depth)) = todo.pop() {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                visited += 1;
                if visited > MAX_ENTRIES || started.elapsed() >= SCAN_BUDGET {
                    return Err(
                        "应用扫描耗时较长，上次列表已保留；可手动选择应用或稍后重新扫描".into(),
                    );
                }
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if is_app(&path) {
                    if let Ok(app) = bundle(&path, false) {
                        if seen.insert(app.root.clone()) {
                            result.push(app);
                        }
                    }
                } else if depth < 2 && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    todo.push((path, depth + 1));
                }
            }
        }
        result.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.root.cmp(&b.root))
        });
        Ok(result)
    })
}

fn mac_binding(binding: &DesktopBinding) -> Result<(&str, &str, Option<&str>), String> {
    match binding {
        DesktopBinding::Macos {
            bundle_id,
            location,
            requirement,
        } => Ok((bundle_id, location, requirement.as_deref())),
        _ => Err("这个应用关联来自其他操作系统，请重新选择本机应用".into()),
    }
}

fn verify_candidate(
    binding: &DesktopBinding,
    path: &Path,
    validate: bool,
) -> Result<DesktopApplication, String> {
    let (bundle_id, _, requirement) = mac_binding(binding)?;
    let mut app = bundle(path, false)?;
    let (current_id, _, current_requirement) = mac_binding(&app.binding)?;
    if current_id != bundle_id || requirement.is_some() != current_requirement.is_some() {
        return Err("应用身份或签名方式已变化，请重新关联".into());
    }
    if validate {
        signature(Path::new(&app.root), true, requirement)?;
    } else if requirement != current_requirement {
        return Err("应用签名信息已变化，请刷新或重新关联；启动前仍会完整核对身份".into());
    }
    // Keep the original requirement, not one supplied by the new candidate.
    if let DesktopBinding::Macos {
        requirement: saved, ..
    } = &mut app.binding
    {
        *saved = requirement.map(str::to_owned);
    }
    Ok(app)
}

pub fn resolve(binding: &DesktopBinding) -> Result<DesktopApplication, String> {
    resolve_impl(binding, true)
}

/// Fast display refresh only. No deep signature verification or trust decision
/// is made here; inspect/save and every actual launch retain strong validation.
pub fn resolve_display(binding: &DesktopBinding) -> Result<DesktopApplication, String> {
    resolve_impl(binding, false)
}

fn resolve_impl(binding: &DesktopBinding, validate: bool) -> Result<DesktopApplication, String> {
    objc2::rc::autoreleasepool(|_| {
        let (bundle_id, location, requirement) = mac_binding(binding)?;
        let original = Path::new(location);
        if fs::symlink_metadata(original).is_ok() {
            return verify_candidate(binding, original, validate);
        }
        if let Ok(relative) = original.strip_prefix("/Volumes") {
            if let Some(volume) = relative.components().next() {
                if !Path::new("/Volumes").join(volume).exists() {
                    return Err("应用所在磁盘未连接，请连接磁盘后刷新；原设置已保留".into());
                }
            }
        }
        if requirement.is_none() {
            return Err("原安装位置未找到应用，请重新关联；原参数和代理设置已保留".into());
        }
        let urls = NSWorkspace::sharedWorkspace()
            .URLsForApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id));
        let paths = urls
            .iter()
            .filter_map(|url| url.path().map(|path| PathBuf::from(path.to_string())))
            .collect::<Vec<_>>();
        resolve_candidates(binding, &paths, validate)
    })
}

fn resolve_candidates(
    binding: &DesktopBinding,
    paths: &[PathBuf],
    validate: bool,
) -> Result<DesktopApplication, String> {
    let mut seen = HashSet::new();
    let mut matches = Vec::new();
    for path in paths {
        if let Ok(app) = verify_candidate(binding, path, validate) {
            if seen.insert(app.root.clone()) {
                matches.push(app);
            }
        }
    }
    match matches.len() {
        1 => {
            let mut app = matches.remove(0);
            app.detail = "应用位置已变化，已找到同一签名身份的唯一副本；原设置已保留".into();
            Ok(app)
        }
        0 => Err("未找到原关联的应用，请确认已安装或重新关联".into()),
        _ => Err("发现多个相同身份的应用，请重新关联并选择安装位置".into()),
    }
}

/// Called on Tauri's main thread. Signature inspection happens afterwards on a
/// worker so slow disks never block the panel's event loop.
pub fn choose() -> Result<Option<String>, String> {
    let mtm = MainThreadMarker::new().ok_or("应用选择窗口需要在主线程打开")?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setTreatsFilePackagesAsDirectories(false);
    panel.setTitle(Some(&NSString::from_str("选择要代理启动的应用")));
    // Available on the macOS 13 baseline; restrict packages without adding a
    // second framework dependency merely to express com.apple.application-bundle.
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
        "app",
    )])));
    if panel.runModal() != 1 {
        return Ok(None);
    }
    let path = panel
        .URL()
        .and_then(|url| url.path())
        .ok_or("没有取得所选应用路径")?
        .to_string();
    if !is_app(Path::new(&path)) {
        return Err("请选择 .app 应用包".into());
    }
    Ok(Some(path))
}

#[derive(Debug, Clone)]
pub struct RunningApplication {
    process_id: u32,
    launch_date: u64,
    bundle_id: String,
    location: PathBuf,
}

impl RunningApplication {
    pub fn pid(&self) -> u32 {
        self.process_id
    }

    pub fn is_running(&self) -> bool {
        objc2::rc::autoreleasepool(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(self.process_id as i32)
                .and_then(|app| record(&app).ok())
                .is_some_and(|app| {
                    app.process_id == self.process_id
                        && app.launch_date == self.launch_date
                        && app.bundle_id == self.bundle_id
                        && app.location == self.location
                })
        })
    }
}

fn record(app: &NSRunningApplication) -> Result<RunningApplication, String> {
    let pid = app.processIdentifier();
    if pid <= 0 || app.isTerminated() {
        return Err("应用在启动过程中退出，请检查系统提示后重试".into());
    }
    let location = app
        .bundleURL()
        .and_then(|url| url.path())
        .ok_or("启动结果没有应用位置")?;
    Ok(RunningApplication {
        process_id: pid as u32,
        launch_date: app
            .launchDate()
            .ok_or("启动结果没有实例时间，状态待确认")?
            .timeIntervalSince1970()
            .to_bits(),
        bundle_id: app
            .bundleIdentifier()
            .ok_or("启动结果没有应用身份")?
            .to_string(),
        location: Path::new(&location.to_string())
            .canonicalize()
            .map_err(|error| error.to_string())?,
    })
}

/// Any same-bundle instance blocks a normal launch: another copy could receive
/// single-instance activation and retain its old (non-proxy) environment.
pub fn running(binding: &DesktopBinding) -> Option<u32> {
    let (bundle_id, _, _) = mac_binding(binding).ok()?;
    objc2::rc::autoreleasepool(|_| {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
            bundle_id,
        ))
        .iter()
        .find(|app| !app.isTerminated() && app.processIdentifier() > 0)
        .map(|app| app.processIdentifier() as u32)
    })
}

/// A native activation may complete after the bounded foreground wait. Keep its
/// receipt separate from the UI request lifetime; no receipt causes a retry.
#[derive(Debug, Clone)]
pub enum LaunchStatus {
    NotTracked,
    Pending,
    Complete(Result<RunningApplication, String>),
}
#[derive(Clone)]
struct LaunchAttempt {
    token: u64,
    receipt: Arc<Mutex<LaunchStatus>>,
}
impl LaunchAttempt {
    fn pending(&self) -> bool {
        self.receipt
            .lock()
            .map_or(true, |status| matches!(*status, LaunchStatus::Pending))
    }
}
#[derive(Debug, Clone)]
pub struct LaunchTicket {
    // The caller and copied Objective-C callback retain the same receipt. It
    // remains queryable even after bounded completed-history eviction.
    receipt: Arc<Mutex<LaunchStatus>>,
}
#[derive(Debug)]
pub enum NativeLaunchOutcome {
    Complete(Result<RunningApplication, String>),
    Pending(LaunchTicket),
}
static LAUNCH_ATTEMPTS: OnceLock<Mutex<HashMap<String, Vec<LaunchAttempt>>>> = OnceLock::new();
static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);

fn attempts() -> &'static Mutex<HashMap<String, Vec<LaunchAttempt>>> {
    LAUNCH_ATTEMPTS.get_or_init(Mutex::default)
}

/// Query the precise attempt, never a newer activation of the same application.
/// Receipts are reusable and remain available after the caller times out.
pub fn launch_status(ticket: &LaunchTicket) -> LaunchStatus {
    ticket
        .receipt
        .lock()
        .map(|status| status.clone())
        .unwrap_or(LaunchStatus::NotTracked)
}

fn begin_attempt(binding: &DesktopBinding) -> Result<LaunchTicket, String> {
    let (id, _, _) = mac_binding(binding)?;
    let mut items = attempts()
        .lock()
        .map_err(|_| "应用启动记录读取失败，请刷新状态")?;
    if items
        .get(id)
        .is_some_and(|history| history.iter().any(LaunchAttempt::pending))
    {
        return Err("此应用已有启动请求，请等待系统回执，不要重复启动".into());
    }
    // Bound completed history, but never evict an unresolved activation.
    if items.len() >= 256 && !items.contains_key(id) {
        let oldest = items
            .iter()
            .filter(|(_, history)| history.iter().all(|attempt| !attempt.pending()))
            .min_by_key(|(_, history)| history.last().map(|attempt| attempt.token))
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            items.remove(&oldest);
        } else {
            return Err("待确认的启动任务较多，请先核对已打开的应用".into());
        }
    }
    let token = NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed);
    let history = items.entry(id.to_owned()).or_default();
    if history.len() >= 8 {
        history.remove(0);
    }
    let receipt = Arc::new(Mutex::new(LaunchStatus::Pending));
    history.push(LaunchAttempt {
        token,
        receipt: receipt.clone(),
    });
    Ok(LaunchTicket { receipt })
}

fn finish_attempt(ticket: &LaunchTicket, result: Result<RunningApplication, String>) {
    if let Ok(mut status) = ticket.receipt.lock() {
        if matches!(*status, LaunchStatus::Pending) {
            *status = LaunchStatus::Complete(result);
        }
    }
}

pub fn launch_tracked(
    binding: &DesktopBinding,
    arguments: &[String],
    environment: &[(String, String)],
) -> NativeLaunchOutcome {
    launch_with_wait(binding, arguments, environment, LAUNCH_WAIT)
        .unwrap_or_else(|error| NativeLaunchOutcome::Complete(Err(error)))
}

#[cfg(test)]
fn launch(
    binding: &DesktopBinding,
    arguments: &[String],
    environment: &[(String, String)],
) -> Result<RunningApplication, String> {
    match launch_tracked(binding, arguments, environment) {
        NativeLaunchOutcome::Complete(result) => result,
        NativeLaunchOutcome::Pending(_) => {
            Err("启动结果待确认，请检查 macOS 提示后刷新状态；本次不会重复发起启动".into())
        }
    }
}

/// Invoke from a blocking worker, never the AppKit main thread. The retained
/// completion block owns only Send data; late completion after timeout is safe.
fn launch_with_wait(
    binding: &DesktopBinding,
    arguments: &[String],
    environment: &[(String, String)],
    wait: Duration,
) -> Result<NativeLaunchOutcome, String> {
    if MainThreadMarker::new().is_some() {
        return Err("应用启动等待需要在后台执行".into());
    }
    objc2::rc::autoreleasepool(|_| {
        let app = resolve(binding)?;
        if running(binding).is_some() {
            return Err("应用仍在后台运行，请先使用 ⌘Q 退出，再从这里启动以应用代理设置".into());
        }
        let (bundle_id, _, _) = mac_binding(&app.binding)?;
        let expected_id = bundle_id.to_owned();
        let expected_path = PathBuf::from(&app.root);
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setAllowsRunningApplicationSubstitution(false);
        configuration.setCreatesNewApplicationInstance(false);
        configuration.setAddsToRecentItems(false);
        configuration.setPromptsUserIfNeeded(true);
        configuration.setArguments(&NSArray::from_retained_slice(
            &arguments
                .iter()
                .map(|arg| NSString::from_str(arg))
                .collect::<Vec<_>>(),
        ));
        let keys: Vec<_> = environment
            .iter()
            .map(|(key, _)| NSString::from_str(key))
            .collect();
        let values: Vec<_> = environment
            .iter()
            .map(|(_, value)| NSString::from_str(value))
            .collect();
        configuration.setEnvironment(&NSDictionary::from_slices(
            &keys.iter().map(|key| &**key).collect::<Vec<_>>(),
            &values.iter().map(|value| &**value).collect::<Vec<_>>(),
        ));
        let application_url = file_url(Path::new(&app.root))?;
        let ticket = begin_attempt(&app.binding)?;
        // Another entry for the same bundle may have finished activation while
        // this request was validating its signature. Recheck after reserving.
        if running(binding).is_some() {
            let message =
                "应用仍在后台运行，请先使用 ⌘Q 退出，再从这里启动以应用代理设置".to_owned();
            finish_attempt(&ticket, Err(message.clone()));
            return Err(message);
        }
        let callback_ticket = ticket.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let requested = NSDate::date().timeIntervalSince1970();
        let completion = RcBlock::new(
            move |running: *mut NSRunningApplication, error: *mut NSError| {
                let result = if let Some(error) = unsafe { error.as_ref() } {
                    Err(format!(
                        "macOS 未完成应用启动：{}（代码 {}）",
                        error.localizedDescription(),
                        error.code()
                    ))
                } else if let Some(running) = unsafe { running.as_ref() } {
                    record(running).and_then(|record| {
                        if record.bundle_id != expected_id || record.location != expected_path {
                            Err("系统返回了另一份应用，启动状态待确认；请检查已有实例".into())
                        } else if f64::from_bits(record.launch_date) < requested {
                            Err("应用已有实例，本次代理设置未应用；请先使用 ⌘Q 退出后重试".into())
                        } else {
                            Ok(record)
                        }
                    })
                } else {
                    Err("系统没有返回应用启动结果，请检查应用是否已打开".into())
                };
                finish_attempt(&callback_ticket, result.clone());
                let _ = sender.try_send(result);
            },
        );
        NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
            &application_url,
            &configuration,
            Some(&completion),
        );
        Ok(match receiver.recv_timeout(wait) {
            Ok(result) => NativeLaunchOutcome::Complete(result),
            Err(_) => NativeLaunchOutcome::Pending(ticket),
        })
    })
}

// Minimal Security.framework bridge. Use a designated requirement rather than a
// code hash: a normal signed update changes bytes but keeps its code identity.
type CfRef = *const c_void;
type SecStaticCodeRef = *mut c_void;
type SecRequirementRef = *mut c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: CfRef);
    fn CFDictionaryGetValue(dictionary: CfRef, key: CfRef) -> CfRef;
    fn CFNumberGetValue(number: CfRef, number_type: isize, value: *mut c_void) -> bool;
}
#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecCodeInfoIdentifier: CfRef;
    static kSecCodeInfoFlags: CfRef;
    fn SecStaticCodeCreateWithPath(path: CfRef, flags: u32, code: *mut SecStaticCodeRef) -> i32;
    fn SecStaticCodeCheckValidity(
        code: SecStaticCodeRef,
        flags: u32,
        requirement: SecRequirementRef,
    ) -> i32;
    fn SecCodeCopySigningInformation(
        code: SecStaticCodeRef,
        flags: u32,
        information: *mut CfRef,
    ) -> i32;
    fn SecCodeCopyDesignatedRequirement(
        code: SecStaticCodeRef,
        flags: u32,
        requirement: *mut SecRequirementRef,
    ) -> i32;
    fn SecRequirementCopyString(
        requirement: SecRequirementRef,
        flags: u32,
        text: *mut CfRef,
    ) -> i32;
    fn SecRequirementCreateWithString(
        text: CfRef,
        flags: u32,
        requirement: *mut SecRequirementRef,
    ) -> i32;
}
struct OwnedCf(CfRef);
impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}
fn checked(status: i32, value: CfRef) -> Result<OwnedCf, String> {
    if status != 0 || value.is_null() {
        Err(format!("读取应用签名信息失败（OSStatus {status}）"))
    } else {
        Ok(OwnedCf(value))
    }
}
fn signature(
    path: &Path,
    validate: bool,
    expected: Option<&str>,
) -> Result<Option<String>, String> {
    let url = file_url(path)?;
    unsafe {
        let mut code = ptr::null_mut();
        let status = SecStaticCodeCreateWithPath(Retained::as_ptr(&url).cast(), 0, &mut code);
        let code = checked(status, code)?;
        let mut info = ptr::null();
        let status = SecCodeCopySigningInformation(code.0.cast_mut(), 0, &mut info);
        let info = checked(status, info)?;
        if CFDictionaryGetValue(info.0, kSecCodeInfoIdentifier).is_null() {
            if expected.is_some() {
                return Err("应用签名已变化，请重新关联".into());
            }
            return Ok(None);
        }
        let flags = CFDictionaryGetValue(info.0, kSecCodeInfoFlags);
        let mut flags_value: i32 = 0;
        if flags.is_null() || !CFNumberGetValue(flags, 3, (&mut flags_value as *mut i32).cast()) {
            return Err("应用签名标记读取失败，请稍后刷新".into());
        }
        let requirement = if let Some(expected) = expected {
            let text = NSString::from_str(expected);
            let mut requirement = ptr::null_mut();
            let status =
                SecRequirementCreateWithString(Retained::as_ptr(&text).cast(), 0, &mut requirement);
            Some(checked(status, requirement)?)
        } else {
            None
        };
        if validate {
            let status = SecStaticCodeCheckValidity(
                code.0.cast_mut(),
                0,
                requirement
                    .as_ref()
                    .map_or(ptr::null_mut(), |value| value.0.cast_mut()),
            );
            if status != 0 {
                return Err(format!("应用签名校验未通过，请确认应用已完整安装；身份变化时请重新关联（OSStatus {status}）"));
            }
        }
        if flags_value & 2 != 0 {
            // kSecCodeSignatureAdhoc is deliberately location-bound.
            if expected.is_some() {
                return Err("应用签名身份已变化，请重新关联".into());
            }
            return Ok(None);
        }
        let mut requirement = ptr::null_mut();
        let status = SecCodeCopyDesignatedRequirement(code.0.cast_mut(), 0, &mut requirement);
        let requirement = checked(status, requirement)?;
        let mut text = ptr::null();
        let status = SecRequirementCopyString(requirement.0.cast_mut(), 0, &mut text);
        let text = checked(status, text)?;
        let value = (&*text.0.cast::<NSString>()).to_string();
        if value.is_empty() {
            return Err("应用签名身份为空，请重新选择应用".into());
        }
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::process::Command;

    struct Fixture {
        directory: tempfile::TempDir,
        root: PathBuf,
        id: String,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("测试 App.app");
            let id = format!(
                "com.serylane.fixture.{}",
                directory
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .replace('.', "")
            );
            fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
            let source = directory.path().join("fixture.m");
            fs::write(
                &source,
                include_str!("../../tests/fixtures/macos_proxy_app.m"),
            )
            .unwrap();
            let output = Command::new("/usr/bin/clang")
                .args([
                    "-fobjc-arc",
                    "-framework",
                    "AppKit",
                    "-framework",
                    "Foundation",
                ])
                .arg(&source)
                .arg("-o")
                .arg(root.join("Contents/MacOS/fixture"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let fixture = Self {
                directory,
                root,
                id,
            };
            fixture.write_info("1.0", "fixture", &fixture.id);
            fixture.sign();
            fixture
        }
        fn write_info(&self, version: &str, executable: &str, id: &str) {
            fs::write(
                self.root.join("Contents/Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
                <key>CFBundleIdentifier</key><string>{id}</string>
                <key>CFBundleName</key><string>测试 App</string>
                <key>CFBundleExecutable</key><string>{executable}</string>
                <key>CFBundlePackageType</key><string>APPL</string>
                <key>CFBundleVersion</key><string>{version}</string>
                <key>CFBundleShortVersionString</key><string>{version}</string>
                <key>LSUIElement</key><true/>
                <key>LSMinimumSystemVersion</key><string>13.0</string>
                </dict></plist>"#
                ),
            )
            .unwrap();
        }
        fn sign(&self) {
            let output = Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&self.root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn same_location_upgrade_reads_fresh_plist_not_bundle_cache() {
        let fixture = Fixture::new();
        let before = inspect(&fixture.root).unwrap();
        assert_eq!(before.version, "1.0");
        assert!(matches!(
            before.binding,
            DesktopBinding::Macos {
                requirement: None,
                ..
            }
        ));
        fixture.write_info("2.0", "fixture", &fixture.id);
        fixture.sign();
        let after = resolve(&before.binding).unwrap();
        assert_eq!(after.version, "2.0");
        assert_eq!(after.root, before.root);
    }

    #[test]
    fn replaced_bundle_identity_requires_relink() {
        let fixture = Fixture::new();
        let before = inspect(&fixture.root).unwrap();
        fixture.write_info("2.0", "fixture", "com.serylane.fixture.other");
        fixture.sign();
        assert!(resolve(&before.binding).unwrap_err().contains("重新关联"));
    }

    #[test]
    fn invalid_executable_path_and_broken_signature_are_rejected() {
        let fixture = Fixture::new();
        let before = inspect(&fixture.root).unwrap();
        fixture.write_info("2.0", "fixture", &fixture.id);
        // Display metadata is cheap; launching still validates sealed resources.
        assert_eq!(resolve_display(&before.binding).unwrap().version, "2.0");
        assert!(resolve(&before.binding).unwrap_err().contains("签名"));
        fixture.write_info("2.0", "../fixture", &fixture.id);
        assert!(inspect(&fixture.root).unwrap_err().contains("越界"));
    }

    #[test]
    fn scan_deduplicates_cask_links_and_never_lists_nested_helpers() {
        let fixture = Fixture::new();
        let nested = fixture.root.join("Contents/Frameworks/Helper.app");
        fs::create_dir_all(nested).unwrap();
        symlink(&fixture.root, fixture.directory.path().join("Cask.app")).unwrap();
        let result = collect_roots(&[fixture.directory.path().to_path_buf()]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].version, "1.0");
    }

    #[test]
    fn adhoc_binding_never_switches_to_another_location() {
        let fixture = Fixture::new();
        let before = inspect(&fixture.root).unwrap();
        fs::rename(&fixture.root, fixture.directory.path().join("Moved.app")).unwrap();
        assert!(resolve(&before.binding).unwrap_err().contains("重新关联"));
    }

    #[test]
    fn duplicate_identity_uses_explicit_copy_and_ambiguous_candidates_are_rejected() {
        let a = Fixture::new();
        let b = Fixture::new();
        b.write_info("9.0", "fixture", &a.id);
        b.sign();
        let original = inspect(&a.root).unwrap();
        assert_eq!(resolve(&original.binding).unwrap().version, "1.0");
        // Candidate matching is independent of registration order or version.
        assert!(
            resolve_candidates(&original.binding, &[a.root.clone(), b.root.clone()], true)
                .unwrap_err()
                .contains("多个")
        );
    }

    #[test]
    fn late_receipt_finishes_pending_and_old_receipts_never_replace_new_attempts() {
        let fixture = Fixture::new();
        let binding = inspect(&fixture.root).unwrap().binding;
        let first = begin_attempt(&binding).unwrap();
        assert!(matches!(launch_status(&first), LaunchStatus::Pending));
        assert!(
            begin_attempt(&binding).is_err(),
            "same application cannot activate concurrently"
        );
        finish_attempt(&first, Err("系统明确取消了启动".into()));
        assert!(matches!(
            launch_status(&first),
            LaunchStatus::Complete(Err(_))
        ));
        let second = begin_attempt(&binding).unwrap();
        finish_attempt(&first, Err("旧回执".into()));
        assert!(matches!(launch_status(&second), LaunchStatus::Pending));
        assert!(
            matches!(launch_status(&first), LaunchStatus::Complete(Err(message)) if message == "系统明确取消了启动")
        );
        finish_attempt(&second, Err("本轮失败".into()));
        assert!(
            matches!(launch_status(&second), LaunchStatus::Complete(Err(message)) if message == "本轮失败")
        );
        for _ in 0..10 {
            let later = begin_attempt(&binding).unwrap();
            finish_attempt(&later, Err("后续明确失败".into()));
        }
        assert!(
            matches!(launch_status(&first), LaunchStatus::Complete(Err(message)) if message == "系统明确取消了启动")
        );
    }

    #[test]
    fn native_launch_preserves_environment_arguments_and_instance_identity() {
        let fixture = Fixture::new();
        let app = inspect(&fixture.root).unwrap();
        let report = fixture.directory.path().join("report.json");
        let exit = fixture.directory.path().join("exit");
        let proxy_before = std::env::var_os("HTTPS_PROXY");
        let environment = vec![
            ("HTTPS_PROXY".into(), "http://127.0.0.1:17892".into()),
            ("NO_PROXY".into(), "localhost,127.0.0.1,::1".into()),
            (
                "SERYLANE_FIXTURE_REPORT".into(),
                report.to_string_lossy().into_owned(),
            ),
            (
                "SERYLANE_FIXTURE_EXIT".into(),
                exit.to_string_lossy().into_owned(),
            ),
        ];
        // Tests only this independently compiled, temporary .app. Its built-in
        // 30-second deadline guarantees cleanup even if an assertion panics.
        let outcome = launch_with_wait(
            &app.binding,
            &["--fixture-argument".into(), "参数 with spaces".into()],
            &environment,
            Duration::ZERO,
        )
        .unwrap();
        let instance = match outcome {
            NativeLaunchOutcome::Complete(result) => result.unwrap(),
            NativeLaunchOutcome::Pending(ticket) => {
                let deadline = Instant::now() + Duration::from_secs(15);
                loop {
                    match launch_status(&ticket) {
                        LaunchStatus::Complete(result) => break result.unwrap(),
                        LaunchStatus::Pending if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(25))
                        }
                        state => panic!("native late receipt did not converge: {state:?}"),
                    }
                }
            }
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !report.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let payload: serde_json::Value =
            serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
        assert_eq!(payload["httpsProxy"], "http://127.0.0.1:17892");
        assert_eq!(payload["noProxy"], "localhost,127.0.0.1,::1");
        assert_eq!(payload["arguments"][1], "--fixture-argument");
        assert_eq!(payload["arguments"][2], "参数 with spaces");
        assert_eq!(payload["bundleId"], fixture.id);
        assert_eq!(payload["pid"].as_u64(), Some(instance.pid() as u64));
        assert_eq!(running(&app.binding), Some(instance.pid()));
        assert!(instance.is_running());
        let mut wrong_instance = instance.clone();
        wrong_instance.launch_date = 0;
        assert!(
            !wrong_instance.is_running(),
            "PID reuse must not count as the original instance"
        );
        assert!(launch(&app.binding, &[], &environment)
            .unwrap_err()
            .contains("⌘Q"));
        assert_eq!(std::env::var_os("HTTPS_PROXY"), proxy_before);
        fs::write(exit, b"exit").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while instance.is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!instance.is_running());
    }
}
