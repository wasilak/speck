use libc::getifaddrs;
use std::ffi::CStr;

pub fn detect_host_mtu() -> u16 {
    detect_mtu_getifaddrs().unwrap_or(1500).max(1500)
}

fn detect_mtu_getifaddrs() -> Option<u16> {
    let mut ifap = std::ptr::null_mut();
    if unsafe { getifaddrs(&mut ifap) } != 0 {
        return None;
    }
    let mut mtu: Option<u16> = None;
    let mut cursor = ifap;
    while !cursor.is_null() {
        let ifa = unsafe { &*cursor };
        if !ifa.ifa_addr.is_null() && unsafe { (*ifa.ifa_addr).sa_family } as i32 == libc::AF_LINK {
            let flags = ifa.ifa_flags as u32;
            if (flags & libc::IFF_UP as u32) != 0
                && (flags & libc::IFF_LOOPBACK as u32) == 0
                && let Ok(name) = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_str()
                && let Some(val) = get_if_mtu_sysctl(name)
            {
                mtu = Some(val.max(mtu.unwrap_or(0)));
            }
        }
        cursor = ifa.ifa_next;
    }
    unsafe { libc::freeifaddrs(ifap) };
    mtu
}

fn get_if_mtu_sysctl(name: &str) -> Option<u16> {
    let sysctl_key = format!("net.interface.{name}.mtu\0");
    let cname = CStr::from_bytes_with_nul(sysctl_key.as_bytes()).ok()?;
    let mut mtu: u32 = 0;
    let mut len = std::mem::size_of::<u32>();
    let ret = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut mtu as *mut u32 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 && len == std::mem::size_of::<u32>() {
        Some(mtu as u16)
    } else {
        None
    }
}

pub fn mss_for_mtu(mtu: u16) -> u16 {
    mtu.saturating_sub(40)
}
