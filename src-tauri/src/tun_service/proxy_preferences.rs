use crate::platform::{MacServiceProxyState, ProxyProtocolState};
use core_foundation::{
    array::CFArray,
    base::{CFType, CFTypeRef, TCFType},
    data::CFData,
    dictionary::{CFDictionary, CFDictionaryRef, CFMutableDictionary},
    number::CFNumber,
    propertylist::{self, kCFPropertyListBinaryFormat_v1_0, kCFPropertyListImmutable},
    string::{CFString, CFStringRef},
};
use std::time::{Duration, Instant};

type Dict = CFDictionary<CFString, CFType>;

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCPreferencesCreate(
        allocator: CFTypeRef,
        name: CFStringRef,
        identifier: CFStringRef,
    ) -> CFTypeRef;
    fn SCPreferencesLock(prefs: CFTypeRef, wait: u8) -> u8;
    fn SCPreferencesUnlock(prefs: CFTypeRef) -> u8;
    fn SCPreferencesCommitChanges(prefs: CFTypeRef) -> u8;
    fn SCPreferencesApplyChanges(prefs: CFTypeRef) -> u8;
    fn SCNetworkServiceCopy(prefs: CFTypeRef, service_id: CFStringRef) -> CFTypeRef;
    fn SCNetworkServiceGetName(service: CFTypeRef) -> CFStringRef;
    fn SCNetworkServiceCopyProtocol(service: CFTypeRef, protocol_type: CFStringRef) -> CFTypeRef;
    fn SCNetworkProtocolGetConfiguration(protocol: CFTypeRef) -> CFDictionaryRef;
    fn SCNetworkProtocolSetConfiguration(protocol: CFTypeRef, config: CFDictionaryRef) -> u8;
    fn SCDynamicStoreCopyValue(store: CFTypeRef, key: CFStringRef) -> CFDictionaryRef;
    fn SCDynamicStoreCopyProxies(store: CFTypeRef) -> CFDictionaryRef;
    fn SCError() -> i32;
}

#[derive(Default)]
pub(super) struct ProxyLease {
    saved: Option<(String, Vec<u8>, Vec<u8>)>,
}

impl ProxyLease {
    pub(super) fn enable(&mut self, port: u16, expected_service: &str) -> Result<(), String> {
        let id = primary_service().ok_or("没有检测到活动网络服务")?;
        if id != expected_service {
            return Err("活动网络服务已变化，请再次切换".into());
        }
        let prefs = Preferences::lock()?;
        let protocol = prefs.proxy_protocol(&id)?;
        let original = configuration(&protocol);
        if let Some((previous_id, _, previous)) = &self.saved {
            if id != *previous_id || original != deserialize(previous)? {
                return Err("系统代理所属网络服务或设置已变化".into());
            }
        }

        let mut applied = copy_dict(&original);
        for key in ["HTTP", "HTTPS", "SOCKS"] {
            applied.set(
                CFString::new(&format!("{key}Enable")),
                CFNumber::from(1).as_CFType(),
            );
            applied.set(
                CFString::new(&format!("{key}Proxy")),
                CFString::new("127.0.0.1").as_CFType(),
            );
            applied.set(
                CFString::new(&format!("{key}Port")),
                CFNumber::from(i32::from(port)).as_CFType(),
            );
        }
        let mut exclusions = exclusions(&original);
        for domain in ["localhost", "127.0.0.1", "::1", "*.local"] {
            let domain = CFString::new(domain);
            if !exclusions.contains(&domain) {
                exclusions.push(domain);
            }
        }
        applied.set(
            CFString::new("ExceptionsList"),
            CFArray::from_CFTypes(&exclusions).as_CFType(),
        );
        let applied = applied.to_immutable();
        let previous = match &self.saved {
            Some((_, original, _)) => original.clone(),
            None => serialized(&original)?,
        };
        // Keep a restoration record even if apply succeeded and readback times
        // out. The App also retains its existing startup recovery snapshot.
        self.saved = Some((id, previous, serialized(&applied)?));
        prefs.write(&protocol, &applied)?;
        wait_effective(|config| matches_loopback(config, port))
    }

    pub(super) fn restore(&mut self) -> Result<(), String> {
        let Some((id, original, applied)) = &self.saved else {
            return Ok(());
        };
        let prefs = Preferences::lock()?;
        let protocol = prefs.proxy_protocol(id)?;
        let saved = deserialize(original)?;
        let current = configuration(&protocol);
        if current == deserialize(applied)? {
            prefs.write(&protocol, &saved)?;
            if primary_service().as_ref() == Some(id) {
                wait_effective(|effective| same_proxy_fields(effective, &saved))?;
            }
        }
        self.saved = None;
        Ok(())
    }
}

impl Drop for ProxyLease {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub(crate) fn loopback_is_active(port: u16) -> bool {
    effective()
        .as_ref()
        .is_some_and(|settings| matches_loopback(settings, port))
}

pub(crate) fn capture_primary_proxy() -> Result<(String, MacServiceProxyState), String> {
    let id = primary_service().ok_or("没有检测到活动网络服务")?;
    let prefs = preferences()?;
    let service = service(&prefs, &id)?;
    let name = unsafe { SCNetworkServiceGetName(service.as_CFTypeRef()) };
    if name.is_null() {
        return Err("无法读取活动服务名称".into());
    }
    let name = unsafe { CFString::wrap_under_get_rule(name) }.to_string();
    let config = configuration(&proxy_protocol(&service)?);
    let read = |kind: &str| ProxyProtocolState {
        enabled: number(&config, &format!("{kind}Enable")) != 0,
        server: string(&config, &format!("{kind}Proxy")).unwrap_or_default(),
        port: number(&config, &format!("{kind}Port"))
            .try_into()
            .unwrap_or_default(),
    };
    Ok((
        id,
        MacServiceProxyState {
            service: name,
            http: read("HTTP"),
            https: read("HTTPS"),
            socks: read("SOCKS"),
            bypass_domains: exclusions(&config)
                .iter()
                .map(ToString::to_string)
                .collect(),
        },
    ))
}

fn exclusions(config: &Dict) -> Vec<CFString> {
    let array = config
        .find(CFString::new("ExceptionsList"))
        .and_then(|value| value.downcast::<CFArray>());
    array
        .map(|array| {
            array
                .iter()
                .filter_map(|value| unsafe {
                    CFType::wrap_under_get_rule(*value).downcast_into::<CFString>()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn matches_loopback(settings: &Dict, port: u16) -> bool {
    ["HTTP", "HTTPS", "SOCKS"].iter().all(|kind| {
        number(settings, &format!("{kind}Enable")) == 1
            && number(settings, &format!("{kind}Port")) == i32::from(port)
            && string(settings, &format!("{kind}Proxy")).as_deref() == Some("127.0.0.1")
    })
}

fn same_proxy_fields(a: &Dict, b: &Dict) -> bool {
    ["HTTP", "HTTPS", "SOCKS"].iter().all(|kind| {
        let enabled = format!("{kind}Enable");
        number(a, &enabled) == number(b, &enabled)
            && (number(b, &enabled) == 0
                || (string(a, &format!("{kind}Proxy")) == string(b, &format!("{kind}Proxy"))
                    && number(a, &format!("{kind}Port")) == number(b, &format!("{kind}Port"))))
    })
}

fn number(dict: &Dict, key: &str) -> i32 {
    dict.find(CFString::new(key))
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|value| value.to_i32())
        .unwrap_or_default()
}

fn string(dict: &Dict, key: &str) -> Option<String> {
    dict.find(CFString::new(key))
        .and_then(|value| value.downcast::<CFString>())
        .map(|value| value.to_string())
}

fn primary_service() -> Option<String> {
    for family in ["IPv4", "IPv6"] {
        let key = CFString::new(&format!("State:/Network/Global/{family}"));
        let raw = unsafe { SCDynamicStoreCopyValue(std::ptr::null(), key.as_concrete_TypeRef()) };
        if !raw.is_null() {
            let config = unsafe { Dict::wrap_under_create_rule(raw) };
            if let Some(id) = string(&config, "PrimaryService") {
                return Some(id);
            }
        }
    }
    None
}

fn effective() -> Option<Dict> {
    let raw = unsafe { SCDynamicStoreCopyProxies(std::ptr::null()) };
    (!raw.is_null()).then(|| unsafe { Dict::wrap_under_create_rule(raw) })
}

fn wait_effective(ready: impl Fn(&Dict) -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(650);
    loop {
        if effective().as_ref().is_some_and(&ready) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("macOS 系统代理回读超时".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn configuration(protocol: &CFType) -> Dict {
    let value = unsafe { SCNetworkProtocolGetConfiguration(protocol.as_CFTypeRef()) };
    if value.is_null() {
        Dict::from_CFType_pairs(&[])
    } else {
        unsafe { Dict::wrap_under_get_rule(value) }
    }
}

fn copy_dict(dict: &Dict) -> CFMutableDictionary<CFString, CFType> {
    let (keys, values) = dict.get_keys_and_values();
    let pairs = keys
        .into_iter()
        .zip(values)
        .map(|(key, value)| unsafe {
            (
                CFString::wrap_under_get_rule(key.cast()),
                CFType::wrap_under_get_rule(value),
            )
        })
        .collect::<Vec<_>>();
    CFMutableDictionary::from_CFType_pairs(&pairs)
}

fn serialized(dict: &Dict) -> Result<Vec<u8>, String> {
    propertylist::create_data(dict.as_CFTypeRef(), kCFPropertyListBinaryFormat_v1_0)
        .map(|data| data.bytes().to_vec())
        .map_err(|error| format!("{error:?}"))
}

fn deserialize(bytes: &[u8]) -> Result<Dict, String> {
    let (value, _) =
        propertylist::create_with_data(CFData::from_buffer(bytes), kCFPropertyListImmutable)
            .map_err(|error| format!("{error:?}"))?;
    let dict = unsafe { CFType::wrap_under_create_rule(value) }
        .downcast_into::<CFDictionary>()
        .ok_or_else(|| "系统代理备份格式不正确".to_string())?;
    Ok(unsafe { Dict::wrap_under_get_rule(dict.as_concrete_TypeRef()) })
}

struct Preferences(CFType);
impl Preferences {
    fn lock() -> Result<Self, String> {
        let prefs = preferences()?;
        check(
            unsafe { SCPreferencesLock(prefs.as_CFTypeRef(), 0) },
            "锁定系统网络偏好",
        )?;
        Ok(Self(prefs))
    }

    fn proxy_protocol(&self, id: &str) -> Result<CFType, String> {
        proxy_protocol(&service(&self.0, id)?)
    }

    fn write(&self, protocol: &CFType, new: &Dict) -> Result<(), String> {
        let prefs = self.0.as_CFTypeRef();
        unsafe {
            check(
                SCNetworkProtocolSetConfiguration(
                    protocol.as_CFTypeRef(),
                    new.as_concrete_TypeRef(),
                ),
                "写入系统代理",
            )?;
            check(SCPreferencesCommitChanges(prefs), "提交系统代理")?;
            check(SCPreferencesApplyChanges(prefs), "应用系统代理")
        }
    }
}

fn preferences() -> Result<CFType, String> {
    let name = CFString::new("Serylane");
    retained(
        unsafe {
            SCPreferencesCreate(
                std::ptr::null(),
                name.as_concrete_TypeRef(),
                std::ptr::null(),
            )
        },
        "读取系统网络偏好",
    )
}

fn service(prefs: &CFType, id: &str) -> Result<CFType, String> {
    let id = CFString::new(id);
    retained(
        unsafe { SCNetworkServiceCopy(prefs.as_CFTypeRef(), id.as_concrete_TypeRef()) },
        "读取活动网络服务",
    )
}

fn proxy_protocol(service: &CFType) -> Result<CFType, String> {
    let proxies = CFString::new("Proxies");
    retained(
        unsafe {
            SCNetworkServiceCopyProtocol(service.as_CFTypeRef(), proxies.as_concrete_TypeRef())
        },
        "读取系统代理协议",
    )
}

impl Drop for Preferences {
    fn drop(&mut self) {
        unsafe {
            SCPreferencesUnlock(self.0.as_CFTypeRef());
        }
    }
}

fn retained(value: CFTypeRef, action: &str) -> Result<CFType, String> {
    if value.is_null() {
        Err(sc_error(action))
    } else {
        Ok(unsafe { CFType::wrap_under_create_rule(value) })
    }
}

fn check(value: u8, action: &str) -> Result<(), String> {
    if value != 0 {
        Ok(())
    } else {
        Err(sc_error(action))
    }
}

fn sc_error(action: &str) -> String {
    format!("{action}失败（SystemConfiguration {}）", unsafe {
        SCError()
    })
}
