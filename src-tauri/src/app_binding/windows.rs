use super::*;
use ::windows::{
    core::{Interface, HSTRING, PWSTR},
    ApplicationModel::{
        Package, PackageCatalog, PackageInstallingEventArgs, PackageStatusChangedEventArgs,
        PackageUninstallingEventArgs, PackageUpdatingEventArgs,
    },
    Data::Xml::Dom::{XmlDocument, XmlElement, XmlLoadSettings},
    Foundation::TypedEventHandler,
    Management::Deployment::{PackageManager, PackageTypes},
    Win32::{
        Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS},
        Storage::Packaging::Appx::{GetApplicationUserModelId, GetPackageFamilyName},
        System::{
            Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
            WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
        },
    },
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

type Updating = Arc<Mutex<HashMap<String, Instant>>>;
type Reply = mpsc::SyncSender<Result<CatalogSnapshot, String>>;
struct Query {
    family: Option<String>,
    fresh: bool,
    reply: Reply,
}

/// One lazy MTA worker, a bounded queue and bounded caller wait. A slow WinRT
/// call never holds the program/configuration lock or spawns unlimited workers.
#[derive(Default)]
pub struct ApplicationCatalog {
    worker: OnceLock<mpsc::SyncSender<Query>>,
}

impl ApplicationCatalog {
    pub fn query(&self, family: Option<&str>, fresh: bool) -> Result<CatalogSnapshot, String> {
        let worker = self.worker.get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Query>(1);
            std::thread::spawn(move || worker_loop(rx));
            tx
        });
        let (tx, rx) = mpsc::sync_channel(1);
        worker
            .try_send(Query {
                family: family.map(str::to_owned),
                fresh,
                reply: tx,
            })
            .map_err(|_| "应用信息正在读取，请稍后重试。".to_string())?;
        rx.recv_timeout(Duration::from_secs(12))
            .map_err(|_| "读取应用信息超时，原配置已保留，请稍后刷新。".to_string())?
    }
}

struct Apartment;
impl Apartment {
    fn new() -> ::windows::core::Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

struct Watcher {
    catalog: PackageCatalog,
    tokens: [i64; 4],
}
impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.catalog.RemovePackageUpdating(self.tokens[0]);
        let _ = self.catalog.RemovePackageInstalling(self.tokens[1]);
        let _ = self.catalog.RemovePackageUninstalling(self.tokens[2]);
        let _ = self.catalog.RemovePackageStatusChanged(self.tokens[3]);
    }
}
fn watch(epoch: Arc<AtomicU64>, updating: Updating) -> ::windows::core::Result<Watcher> {
    let catalog = PackageCatalog::OpenForCurrentUser()?;
    let mut watcher = Watcher {
        catalog: catalog.clone(),
        tokens: [0; 4],
    };
    let e = epoch.clone();
    watcher.tokens[0] = catalog.PackageUpdating(&TypedEventHandler::<
        PackageCatalog,
        PackageUpdatingEventArgs,
    >::new(move |_, args| {
        e.fetch_add(1, Ordering::Relaxed);
        if let Some(args) = args.as_ref() {
            if let Ok(family) = args
                .TargetPackage()
                .and_then(|p| p.Id())
                .and_then(|p| p.FamilyName())
            {
                if let Ok(mut map) = updating.lock() {
                    let key = family.to_string().to_lowercase();
                    if args.IsComplete().unwrap_or(false) {
                        map.remove(&key);
                    } else {
                        map.insert(key, Instant::now());
                    }
                }
            }
        }
        Ok(())
    }))?;
    let e = epoch.clone();
    watcher.tokens[1] = catalog.PackageInstalling(&TypedEventHandler::<
        PackageCatalog,
        PackageInstallingEventArgs,
    >::new(move |_, _| {
        e.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }))?;
    let e = epoch.clone();
    watcher.tokens[2] = catalog.PackageUninstalling(&TypedEventHandler::<
        PackageCatalog,
        PackageUninstallingEventArgs,
    >::new(move |_, _| {
        e.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }))?;
    watcher.tokens[3] = catalog.PackageStatusChanged(&TypedEventHandler::<
        PackageCatalog,
        PackageStatusChangedEventArgs,
    >::new(move |_, _| {
        epoch.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }))?;
    Ok(watcher)
}

fn worker_loop(rx: mpsc::Receiver<Query>) {
    let _apartment = match Apartment::new() {
        Ok(value) => value,
        Err(error) => {
            for query in rx {
                let _ = query
                    .reply
                    .send(Err(format!("Windows 应用服务初始化失败：{error}")));
            }
            return;
        }
    };
    let epoch = Arc::new(AtomicU64::new(0));
    let updating: Updating = Arc::new(Mutex::new(HashMap::new()));
    // The TTL and mandatory fresh launch resolution remain the fallback if the
    // OS does not expose package events to this desktop process.
    let _watcher = watch(epoch.clone(), updating.clone()).ok();
    let mut cache: HashMap<Option<String>, (u64, Instant, CatalogSnapshot)> = HashMap::new();
    for query in rx {
        let key = query.family.as_ref().map(|v| v.to_lowercase());
        let generation = epoch.load(Ordering::Relaxed);
        if !query.fresh {
            if let Some((cached_epoch, at, value)) = cache.get(&key) {
                if *cached_epoch == generation && at.elapsed() < Duration::from_secs(30) {
                    let _ = query.reply.send(Ok(value.clone()));
                    continue;
                }
            }
        }
        let result = collect(query.family.as_deref(), &updating);
        if let Ok(value) = &result {
            if cache.len() > 128 {
                cache.clear();
            }
            cache.insert(key, (generation, Instant::now(), value.clone()));
        }
        let _ = query.reply.send(result);
    }
}

fn collect(family: Option<&str>, updating: &Updating) -> Result<CatalogSnapshot, String> {
    let manager = PackageManager::new().map_err(|e| format!("应用注册服务读取失败：{e}"))?;
    let packages = match family {
        Some(family) => manager.FindPackagesByUserSecurityIdPackageFamilyNameWithPackageTypes(
            &HSTRING::new(),
            &HSTRING::from(family),
            PackageTypes::Main,
        ),
        None => manager
            .FindPackagesByUserSecurityIdWithPackageTypes(&HSTRING::new(), PackageTypes::Main),
    }
    .map_err(|e| format!("当前用户的应用注册信息读取失败：{e}"))?;
    let mut output = CatalogSnapshot::default();
    // Explicit iterator access preserves enumeration errors instead of silently
    // truncating a list and claiming that an application has been uninstalled.
    let iterator = packages.First().map_err(|e| e.to_string())?;
    while iterator.HasCurrent().map_err(|e| e.to_string())? {
        let package = iterator.Current().map_err(|e| e.to_string())?;
        if output.packages.len() >= 4096 {
            return Err("已安装应用数量超过读取上限。".into());
        }
        if let Err(error) = read_package(&package, updating, &mut output) {
            output.warnings.push(error);
        }
        iterator.MoveNext().map_err(|e| e.to_string())?;
    }
    if let Some(family) = family {
        if output.packages.is_empty()
            && updating
                .lock()
                .ok()
                .and_then(|m| m.get(&family.to_lowercase()).copied())
                .is_some_and(|at| at.elapsed() < Duration::from_secs(600))
        {
            output.packages.push(PackageRecord {
                family: family.into(),
                full_name: String::new(),
                availability: AppAvailability::Updating,
                detail: "Windows 正在更新此应用，完成后请刷新重试。".into(),
            });
        }
    }
    output
        .applications
        .sort_by_key(|a| (a.name.to_lowercase(), a.binding.aumid()));
    Ok(output)
}

fn read_package(
    package: &Package,
    updating: &Updating,
    out: &mut CatalogSnapshot,
) -> Result<(), String> {
    let read = || -> ::windows::core::Result<_> {
        let id = package.Id()?;
        let family = id.FamilyName()?.to_string();
        let full_name = id.FullName()?.to_string();
        let v = id.Version()?;
        Ok((
            family,
            full_name,
            format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision),
        ))
    };
    let (family, full_name, version) = read().map_err(|e| format!("部分应用身份读取失败：{e}"))?;
    let mut record = PackageRecord {
        family: family.clone(),
        full_name: full_name.clone(),
        availability: AppAvailability::Ready,
        detail: String::new(),
    };
    let status = package.Status().map_err(|e| e.to_string());
    let result = (|| -> Result<Vec<InstalledApplication>, String> {
        let status = status?;
        if status.DeploymentInProgress().map_err(|e| e.to_string())? {
            let known_update = updating
                .lock()
                .ok()
                .and_then(|m| m.get(&family.to_lowercase()).copied())
                .is_some_and(|at| at.elapsed() < Duration::from_secs(600));
            record.availability = if known_update {
                AppAvailability::Updating
            } else {
                AppAvailability::Maintenance
            };
            record.detail = if known_update {
                "Windows 正在更新此应用，完成后请刷新重试。"
            } else {
                "Windows 正在安装、更新或维护此应用，请稍后刷新。"
            }
            .into();
            return Ok(vec![]);
        }
        if !status.VerifyIsOK().map_err(|e| e.to_string())? {
            record.availability = AppAvailability::ReadError;
            record.detail = "Windows 报告应用安装状态异常，请检查应用安装或修复后重试。".into();
            return Ok(vec![]);
        }
        let installed = package
            .InstalledLocation()
            .and_then(|f| f.Path())
            .map_err(|e| e.to_string())?
            .to_string();
        let root = package
            .EffectivePath()
            .map(|p| p.to_string())
            .unwrap_or_else(|_| installed.clone());
        let manifest_path = std::path::Path::new(&installed).join("AppxManifest.xml");
        if std::fs::metadata(&manifest_path)
            .map_err(|e| e.to_string())?
            .len()
            > 2 * 1024 * 1024
        {
            return Err("应用清单超过读取上限".into());
        }
        let xml = std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        let mut names = BTreeMap::new();
        if let Ok(entries) = package.GetAppListEntriesAsync().and_then(|op| op.get()) {
            for entry in entries {
                if let (Ok(aumid), Ok(name)) = (
                    entry.AppUserModelId(),
                    entry.DisplayInfo().and_then(|d| d.DisplayName()),
                ) {
                    names.insert(aumid.to_string(), name.to_string());
                }
            }
        }
        let fallback = package
            .DisplayName()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| family.clone());
        manifest_apps(
            &xml, &family, &full_name, &version, &root, &names, &fallback,
        )
    })();
    match result {
        Ok(apps) => out.applications.extend(apps),
        Err(error) => {
            record.availability = AppAvailability::ReadError;
            record.detail = format!("应用信息读取失败，请刷新重试。详情：{error}");
        }
    }
    if record.availability == AppAvailability::ReadError {
        out.warnings
            .push(format!("{}：应用信息读取失败", record.family));
    }
    out.packages.push(record);
    Ok(())
}

fn manifest_apps(
    xml: &str,
    family: &str,
    full_name: &str,
    version: &str,
    root: &str,
    names: &BTreeMap<String, String>,
    fallback: &str,
) -> Result<Vec<InstalledApplication>, String> {
    (|| -> ::windows::core::Result<_> {
        let document = XmlDocument::new()?;
        let settings = XmlLoadSettings::new()?;
        settings.SetProhibitDtd(true)?; settings.SetResolveExternals(false)?;
        document.LoadXmlWithSettings(&HSTRING::from(xml), &settings)?;
        let nodes = document.SelectNodes(&HSTRING::from("/*[local-name()='Package']/*[local-name()='Applications']/*[local-name()='Application']"))?;
        let mut apps = vec![];
        for index in 0..nodes.Length()? {
            let element: XmlElement = nodes.Item(index)?.cast()?;
            let attr = |name: &str| element.SelectSingleNode(&HSTRING::from(format!("@*[local-name()='{name}']"))).and_then(|n| n.InnerText()).map(|s| s.to_string()).unwrap_or_default();
            let binding = AppBinding { package_family_name: family.into(), application_id: attr("Id") };
            if !binding.valid() { continue; }
            let relative = attr("Executable");
            let behavior = attr("RuntimeBehavior");
            let full_trust_entry = attr("EntryPoint").eq_ignore_ascii_case("windows.fullTrustApplication");
            let trust = attr("TrustLevel");
            let classic = match behavior.as_str() {
                "" => full_trust_entry && trust != "appContainer",
                "packagedClassicApp" | "win32App" => trust == "mediumIL" || (trust.is_empty() && full_trust_entry),
                _ => false,
            };
            let path_valid = relative_windows_path(&relative) && relative.to_ascii_lowercase().ends_with(".exe");
            let executable = if path_valid { std::path::Path::new(root).join(&relative).to_string_lossy().into_owned() } else { String::new() };
            // Activation-only/hosted apps and manifest-owned launch arguments
            // need a different adapter; never silently drop them or fake proxying.
            let supported = classic && path_valid && attr("Parameters").is_empty() && attr("CurrentDirectoryPath").is_empty() && attr("HostId").is_empty();
            let (availability, detail) = if !supported {
                (AppAvailability::UnsupportedLaunch, "此应用使用系统激活或专用启动参数，当前代理启动方式尚未适配。")
            } else if !std::path::Path::new(&executable).is_file() {
                (AppAvailability::MissingFile, "应用入口文件暂未就绪，请刷新或检查安装状态。")
            } else { (AppAvailability::Ready, "已关联应用，更新后自动定位当前安装版本。") };
            let name = names.get(&binding.aumid()).cloned().unwrap_or_else(|| format!("{fallback} · {}", binding.application_id));
            apps.push(InstalledApplication { binding, name, version: version.into(), package_full_name: full_name.into(), package_root: root.into(), executable, availability, detail: detail.into() });
        }
        Ok(apps)
    })().map_err(|e| e.to_string())
}

pub fn running_bound(binding: &AppBinding) -> Option<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let expected = binding.aumid();
    system.processes().values().find_map(|process| unsafe {
        let pid = process.pid().as_u32();
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 0;
        let mut found = false;
        if GetApplicationUserModelId(handle, &mut len, None) == ERROR_INSUFFICIENT_BUFFER
            && len <= 1024
        {
            let mut buffer = vec![0u16; len as usize];
            if GetApplicationUserModelId(handle, &mut len, Some(PWSTR(buffer.as_mut_ptr())))
                == ERROR_SUCCESS
            {
                let id = String::from_utf16_lossy(&buffer[..len.saturating_sub(1) as usize]);
                found = id.eq_ignore_ascii_case(&expected);
            }
        } else {
            // Some full-trust subprocesses have package identity but no AUMID.
            // Conservatively ask the user to exit that package's running app.
            len = 0;
            if GetPackageFamilyName(handle, &mut len, None) == ERROR_INSUFFICIENT_BUFFER
                && len <= 1024
            {
                let mut buffer = vec![0u16; len as usize];
                if GetPackageFamilyName(handle, &mut len, Some(PWSTR(buffer.as_mut_ptr())))
                    == ERROR_SUCCESS
                {
                    found = String::from_utf16_lossy(&buffer[..len.saturating_sub(1) as usize])
                        .eq_ignore_ascii_case(&binding.package_family_name);
                }
            }
        }
        let _ = CloseHandle(handle);
        found.then_some(pid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_manifest_parser_preserves_application_ids_and_launch_capabilities() {
        let _apartment = Apartment::new().unwrap();
        let xml = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"><Applications><Application Id="App" Executable="app.exe" EntryPoint="Windows.FullTrustApplication"/><Application Id="Other" Executable="other.exe" EntryPoint="Hosted.App"/></Applications></Package>"#;
        let apps = manifest_apps(
            xml,
            "Example.App_123456789abcd",
            "full",
            "1.0.0.0",
            "C:\\fixture",
            &BTreeMap::new(),
            "Example",
        )
        .unwrap();
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].availability, AppAvailability::MissingFile);
        assert_eq!(apps[1].availability, AppAvailability::UnsupportedLaunch);
        let traversal = xml.replace("app.exe", "..\\app.exe");
        assert_eq!(
            manifest_apps(
                &traversal,
                "Example.App_123456789abcd",
                "full",
                "1",
                "C:\\fixture",
                &BTreeMap::new(),
                "Example"
            )
            .unwrap()[0]
                .availability,
            AppAvailability::UnsupportedLaunch
        );
    }
    #[test]
    fn native_catalog_reads_current_user_without_powershell() {
        let result = ApplicationCatalog::default().query(None, true).unwrap();
        for app in &result.applications {
            assert!(app.binding.valid());
        }
    }
}
