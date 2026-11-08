use super::{Error, Result};
use portal_core::{config::Network, engine::LocalClock};
use serde::Serialize;
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    net::Ipv4Addr,
    ptr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, HANDLE, SYSTEMTIME},
    NetworkManagement::{
        IpHelper::*,
        Ndis::{IfOperStatusUp, NET_LUID_LH},
        WiFi::*,
    },
    Networking::WinSock::*,
    System::SystemInformation::GetLocalTime,
};

pub struct Signal {
    pub notify: Notify,
    pub schedule: Notify,
    epoch: AtomicU64,
    token: Mutex<CancellationToken>,
}
impl Signal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            notify: Notify::new(),
            schedule: Notify::new(),
            epoch: AtomicU64::new(1),
            token: Mutex::new(CancellationToken::new()),
        })
    }
    pub fn bump(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel();
        self.notify.notify_one();
    }
    pub fn generation(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    pub fn prepare(&self) -> CancellationToken {
        let mut token = self
            .token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *token = CancellationToken::new();
        token.clone()
    }
}

#[derive(Clone, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub interface_index: Option<u32>,
    pub ipv4: Option<Ipv4Addr>,
    pub route_ready: bool,
    pub is_wifi: bool,
    pub ssid: Option<String>,
    pub wifi_system_profile: Option<bool>,
    pub wifi_auto_connect: Option<bool>,
    pub dns: Vec<Ipv4Addr>,
    pub possible_tunnel_route: bool,
}
impl Snapshot {
    pub fn same_link(&self, other: &Self) -> bool {
        self.interface_index == other.interface_index
            && self.ipv4 == other.ipv4
            && self.route_ready == other.route_ready
            && self.is_wifi == other.is_wifi
            && self.ssid == other.ssid
    }
    pub fn network(&self, generation: u64) -> Network {
        Network {
            generation,
            ipv4: self.ipv4,
            route_ready: self.route_ready,
            is_wifi: self.is_wifi,
            ssid: self.ssid.clone(),
        }
    }
    fn empty() -> Self {
        Self {
            interface_index: None,
            ipv4: None,
            route_ready: false,
            is_wifi: false,
            ssid: None,
            wifi_system_profile: None,
            wifi_auto_connect: None,
            dns: Vec::new(),
            possible_tunnel_route: false,
        }
    }
}

unsafe fn address(value: SOCKET_ADDRESS) -> Option<Ipv4Addr> {
    if value.lpSockaddr.is_null()
        || value.iSockaddrLength < size_of::<SOCKADDR_IN>() as i32
        || (*value.lpSockaddr).sa_family != AF_INET
    {
        return None;
    }
    Some(Ipv4Addr::from(
        (*(value.lpSockaddr as *const SOCKADDR_IN))
            .sin_addr
            .S_un
            .S_addr
            .to_ne_bytes(),
    ))
}
fn inet(ip: Ipv4Addr) -> SOCKADDR_INET {
    let mut value = unsafe { zeroed::<SOCKADDR_INET>() };
    value.Ipv4.sin_family = AF_INET;
    value.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(ip.octets());
    value
}
fn usable(ip: Ipv4Addr) -> bool {
    let a = ip.octets();
    a[0] != 0 && a[0] != 127 && !(a[0] == 169 && a[1] == 254) && a[0] < 224
}

pub fn snapshot(required_ssid: Option<&str>) -> Result<Snapshot> {
    Ok(inventory(required_ssid)?.0)
}
pub fn snapshot_for(required_ssid: Option<&str>, index: Option<u32>) -> Result<Snapshot> {
    let (selected, all) = inventory(required_ssid)?;
    Ok(if let Some(index) = index {
        all.into_iter()
            .find(|n| n.interface_index == Some(index))
            .unwrap_or_else(Snapshot::empty)
    } else {
        selected
    })
}
pub fn interfaces() -> Result<Vec<Snapshot>> {
    Ok(inventory(None)?
        .1
        .into_iter()
        .filter(|n| n.route_ready)
        .collect())
}
fn inventory(required_ssid: Option<&str>) -> Result<(Snapshot, Vec<Snapshot>)> {
    let mut length = 16 * 1024u32;
    let mut buffer = Vec::<u64>::new();
    for _ in 0..4 {
        if length > 1024 * 1024 {
            return Err(Error::Invalid);
        }
        buffer.resize((length as usize).div_ceil(size_of::<u64>()), 0);
        let code = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_INCLUDE_GATEWAYS,
                ptr::null(),
                buffer.as_mut_ptr().cast(),
                &mut length,
            )
        };
        if code == ERROR_BUFFER_OVERFLOW {
            continue;
        }
        if code == ERROR_NO_DATA {
            return Ok((Snapshot::empty(), Vec::new()));
        }
        if code != 0 {
            return Err(std::io::Error::from_raw_os_error(code as i32).into());
        }
        let destination = inet(Ipv4Addr::new(223, 5, 5, 5));
        let mut global_route = unsafe { zeroed::<MIB_IPFORWARD_ROW2>() };
        let mut global_source = unsafe { zeroed::<SOCKADDR_INET>() };
        let global_ok = unsafe {
            GetBestRoute2(
                ptr::null(),
                0,
                ptr::null(),
                &destination,
                0,
                &mut global_route,
                &mut global_source,
            )
        } == 0;
        let mut candidates = Vec::new();
        let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !adapter.is_null() {
            let current = unsafe { &*adapter };
            adapter = current.Next;
            if current.OperStatus != IfOperStatusUp
                || !matches!(current.IfType, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211)
            {
                continue;
            }
            let mut row = unsafe { zeroed::<MIB_IF_ROW2>() };
            row.InterfaceLuid = current.Luid;
            if unsafe { GetIfEntry2(&mut row) } != 0
                || row.InterfaceAndOperStatusFlags._bitfield & 1 == 0
            {
                continue;
            }
            let mut ip = None;
            let mut unicast = current.FirstUnicastAddress;
            while !unicast.is_null() {
                let item = unsafe { &*unicast };
                unicast = item.Next;
                if item.DadState == IpDadStatePreferred {
                    if let Some(value) = unsafe { address(item.Address) }.filter(|ip| usable(*ip)) {
                        ip = Some(value);
                        break;
                    }
                }
            }
            let Some(ip) = ip else {
                continue;
            };
            let source = inet(ip);
            let mut route = unsafe { zeroed::<MIB_IPFORWARD_ROW2>() };
            let mut route_source = unsafe { zeroed::<SOCKADDR_INET>() };
            let ready = unsafe {
                GetBestRoute2(
                    &current.Luid,
                    0,
                    &source,
                    &destination,
                    0,
                    &mut route,
                    &mut route_source,
                )
            } == 0;
            let is_wifi = current.IfType == IF_TYPE_IEEE80211;
            let wifi = if is_wifi {
                wifi_info(&current.Luid)
            } else {
                (None, None, None)
            };
            let mut dns = Vec::new();
            let mut server = current.FirstDnsServerAddress;
            while !server.is_null() {
                let entry = unsafe { &*server };
                server = entry.Next;
                if let Some(ip) = unsafe { address(entry.Address) }.filter(|ip| usable(*ip)) {
                    if !dns.contains(&ip) {
                        dns.push(ip);
                    }
                }
            }
            let index = unsafe { current.Anonymous1.Anonymous.IfIndex };
            let score = current.Ipv4Metric as u64
                + if ready {
                    route.Metric as u64
                } else {
                    u32::MAX as u64
                };
            candidates.push((
                score,
                Snapshot {
                    interface_index: Some(index),
                    ipv4: Some(ip),
                    route_ready: ready,
                    is_wifi,
                    ssid: wifi.0,
                    wifi_system_profile: wifi.1,
                    wifi_auto_connect: wifi.2,
                    dns,
                    possible_tunnel_route: false,
                },
            ));
        }
        candidates.sort_by_key(|(score, value)| (*score, value.interface_index));
        let selected = if let Some(ssid) = required_ssid {
            candidates
                .iter()
                .find(|(_, item)| item.is_wifi && item.ssid.as_deref() == Some(ssid))
                .or_else(|| candidates.iter().find(|(_, item)| item.is_wifi))
        } else {
            candidates
                .iter()
                .find(|(_, item)| {
                    global_ok && item.interface_index == Some(global_route.InterfaceIndex)
                })
                .or_else(|| candidates.first())
        };
        let mut result = selected
            .map(|(_, item)| item.clone())
            .unwrap_or_else(Snapshot::empty);
        result.possible_tunnel_route = global_ok
            && !candidates
                .iter()
                .any(|(_, item)| item.interface_index == Some(global_route.InterfaceIndex));
        return Ok((result, candidates.into_iter().map(|(_, n)| n).collect()));
    }
    Err(Error::Invalid)
}

struct Wifi(HANDLE);
impl Wifi {
    fn open() -> Option<Self> {
        let mut version = 0;
        let mut handle = ptr::null_mut();
        (unsafe { WlanOpenHandle(2, ptr::null(), &mut version, &mut handle) } == 0)
            .then_some(Self(handle))
    }
}
impl Drop for Wifi {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.0, ptr::null());
        }
    }
}
fn wifi_info(luid: &NET_LUID_LH) -> (Option<String>, Option<bool>, Option<bool>) {
    let Some(wifi) = Wifi::open() else {
        return (None, None, None);
    };
    let mut guid = unsafe { zeroed() };
    if unsafe { ConvertInterfaceLuidToGuid(luid, &mut guid) } != 0 {
        return (None, None, None);
    }
    let mut data = ptr::null_mut();
    let mut size = 0;
    if unsafe {
        WlanQueryInterface(
            wifi.0,
            &guid,
            wlan_intf_opcode_current_connection,
            ptr::null(),
            &mut size,
            &mut data,
            ptr::null_mut(),
        )
    } != 0
    {
        return (None, None, None);
    }
    if size < size_of::<WLAN_CONNECTION_ATTRIBUTES>() as u32 {
        unsafe {
            WlanFreeMemory(data);
        }
        return (None, None, None);
    }
    let connection = unsafe { *(data as *const WLAN_CONNECTION_ATTRIBUTES) };
    unsafe {
        WlanFreeMemory(data);
    }
    if connection.isState != wlan_interface_state_connected {
        return (None, None, None);
    }
    let ssid = connection.wlanAssociationAttributes.dot11Ssid;
    let ssid = if ssid.uSSIDLength <= 32 {
        std::str::from_utf8(&ssid.ucSSID[..ssid.uSSIDLength as usize])
            .ok()
            .map(str::to_owned)
    } else {
        None
    };
    let mut xml = ptr::null_mut();
    let mut flags = 0;
    let mut granted = 0;
    if unsafe {
        WlanGetProfile(
            wifi.0,
            &guid,
            connection.strProfileName.as_ptr(),
            ptr::null(),
            &mut xml,
            &mut flags,
            &mut granted,
        )
    } != 0
    {
        return (ssid, None, None);
    }
    let mut n = 0;
    unsafe {
        while n < 65536 && *xml.add(n) != 0 {
            n += 1;
        }
    }
    let text = zeroize::Zeroizing::new(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(xml, n)
    }));
    unsafe {
        WlanFreeMemory(xml.cast());
    }
    let auto = text
        .split_once("<connectionMode>")
        .and_then(|(_, tail)| tail.split_once("</connectionMode>"))
        .and_then(|(mode, _)| match mode.trim() {
            "auto" => Some(true),
            "manual" => Some(false),
            _ => None,
        });
    (ssid, Some(flags & WLAN_PROFILE_USER == 0), auto)
}

pub fn local_clock() -> LocalClock {
    let mut time = unsafe { zeroed::<SYSTEMTIME>() };
    unsafe {
        GetLocalTime(&mut time);
    }
    LocalClock {
        date: time.wYear as u32 * 10000 + time.wMonth as u32 * 100 + time.wDay as u32,
        hour: time.wHour as u8,
    }
}

pub struct Events {
    handles: Vec<HANDLE>,
    wifi: Option<Wifi>,
    _context: Arc<Signal>,
    pub healthy: bool,
}
impl Events {
    pub fn register(signal: Arc<Signal>) -> Self {
        let context = signal;
        let raw = Arc::as_ptr(&context).cast::<c_void>();
        let mut handles = Vec::new();
        let mut healthy = true;
        unsafe {
            let mut handle = ptr::null_mut();
            if NotifyRouteChange2(AF_INET, Some(route_changed), raw, 0, &mut handle) == 0 {
                handles.push(handle);
            } else {
                healthy = false;
            }
            if NotifyIpInterfaceChange(AF_INET, Some(interface_changed), raw, 0, &mut handle) == 0 {
                handles.push(handle);
            } else {
                healthy = false;
            }
            if NotifyUnicastIpAddressChange(AF_INET, Some(address_changed), raw, 0, &mut handle)
                == 0
            {
                handles.push(handle);
            } else {
                healthy = false;
            }
        }
        let wifi = Wifi::open();
        if let Some(wifi) = &wifi {
            if unsafe {
                WlanRegisterNotification(
                    wifi.0,
                    WLAN_NOTIFICATION_SOURCE_ACM,
                    1,
                    Some(wifi_changed),
                    raw,
                    ptr::null(),
                    ptr::null_mut(),
                )
            } != 0
            {
                healthy = false;
            }
        }
        Self {
            handles,
            wifi,
            _context: context,
            healthy,
        }
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        unsafe {
            for handle in &self.handles {
                CancelMibChangeNotify2(*handle);
            }
            if let Some(wifi) = &self.wifi {
                WlanRegisterNotification(
                    wifi.0,
                    WLAN_NOTIFICATION_SOURCE_NONE,
                    1,
                    None,
                    ptr::null(),
                    ptr::null(),
                    ptr::null_mut(),
                );
            }
            self.wifi.take();
        }
    }
}
unsafe extern "system" fn route_changed(
    context: *const c_void,
    _: *const MIB_IPFORWARD_ROW2,
    _: MIB_NOTIFICATION_TYPE,
) {
    (*(context as *const Signal)).bump();
}
unsafe extern "system" fn interface_changed(
    context: *const c_void,
    _: *const MIB_IPINTERFACE_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    (*(context as *const Signal)).bump();
}
unsafe extern "system" fn address_changed(
    context: *const c_void,
    _: *const MIB_UNICASTIPADDRESS_ROW,
    _: MIB_NOTIFICATION_TYPE,
) {
    (*(context as *const Signal)).bump();
}
unsafe extern "system" fn wifi_changed(data: *mut L2_NOTIFICATION_DATA, context: *mut c_void) {
    if data.is_null() {
        return;
    }
    let code = (*data).NotificationCode as i32;
    if [
        wlan_notification_acm_connection_start,
        wlan_notification_acm_connection_complete,
        wlan_notification_acm_disconnected,
        wlan_notification_acm_interface_arrival,
        wlan_notification_acm_interface_removal,
        wlan_notification_acm_profile_change,
    ]
    .contains(&code)
    {
        (*(context as *const Signal)).bump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_snapshot_is_read_only_and_has_consistent_core_view() {
        let value = snapshot(None).unwrap();
        assert_eq!(value.network(9).generation, 9);
        assert_eq!(value.ipv4, value.network(9).ipv4);
    }
    #[test]
    fn dns_and_profile_changes_do_not_restart_physical_link_stability() {
        let a = Snapshot {
            interface_index: Some(21),
            ipv4: Some("10.1.2.3".parse().unwrap()),
            route_ready: true,
            is_wifi: true,
            ssid: Some("Campus".into()),
            ..Snapshot::empty()
        };
        let mut b = a.clone();
        b.dns = vec!["223.5.5.5".parse().unwrap()];
        b.wifi_auto_connect = Some(true);
        assert!(a.same_link(&b));
        b.interface_index = Some(22);
        assert!(!a.same_link(&b));
        b.interface_index = a.interface_index;
        b.ipv4 = Some("10.1.2.4".parse().unwrap());
        assert!(!a.same_link(&b));
    }
    #[test]
    fn event_signal_cancels_current_round_immediately() {
        let signal = Signal::new();
        let token = signal.prepare();
        signal.bump();
        assert!(token.is_cancelled());
        assert_eq!(signal.generation(), 2);
        assert!(!signal.prepare().is_cancelled());
        let token = signal.prepare();
        let generation = signal.generation();
        signal.schedule.notify_one();
        assert!(!token.is_cancelled());
        assert_eq!(signal.generation(), generation);
    }
}
