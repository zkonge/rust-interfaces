use std::ffi::CStr;
use std::io::{Error, ErrorKind};
use std::net::IpAddr;
use std::{net, ptr};

use libc::{sockaddr_in, sockaddr_in6};
use nix::sys::socket::AddressFamily;

use crate::{Interface, Kind, NextHop};

// https://github.com/Exa-Networks/exaproxy/blob/master/lib/exaproxy/util/interfaces.py

pub const AF_INET: AddressFamily = nix::sys::socket::AddressFamily::Inet;
pub const AF_INET6: AddressFamily = nix::sys::socket::AddressFamily::Inet6;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
pub const AF_LINK: AddressFamily = nix::sys::socket::AddressFamily::Link;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub const AF_PACKET: AddressFamily = nix::sys::socket::AddressFamily::Packet;

#[allow(dead_code, non_camel_case_types)]
#[repr(C)]
pub enum SIOCGIFFLAGS {
    IFF_UP = 0x1,           /* Interface is up.  */
    IFF_BROADCAST = 0x2,    /* Broadcast address valid.  */
    IFF_DEBUG = 0x4,        /* Turn on debugging.  */
    IFF_LOOPBACK = 0x8,     /* Is a loopback net.  */
    IFF_POINTOPOINT = 0x10, /* Interface is point-to-point link.  */
    IFF_NOTRAILERS = 0x20,  /* Avoid use of trailers.  */
    IFF_RUNNING = 0x40,     /* Resources allocated.  */
    IFF_NOARP = 0x80,       /* No address resolution protocol.  */
    IFF_PROMISC = 0x100,    /* Receive all packets.  */

    /* Not supported */
    IFF_ALLMULTI = 0x200, /* Receive all multicast packets.  */

    IFF_MASTER = 0x400, /* Master of a load balancer.  */
    IFF_SLAVE = 0x800,  /* Slave of a load balancer.  */

    IFF_MULTICAST = 0x1000, /* Supports multicast.  */

    IFF_PORTSEL = 0x2000,   /* Can set media type.  */
    IFF_AUTOMEDIA = 0x4000, /* Auto media select active.  */
    IFF_DYNAMIC = 0x8000,   /* Dialup device with changing addresses.  */
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct union_ifa_ifu {
    pub data: *mut std::os::raw::c_void,
}
impl union_ifa_ifu {
    pub fn ifu_broadaddr(&mut self) -> *mut nix::sys::socket::sockaddr {
        self.data as *mut nix::sys::socket::sockaddr
    }
    pub fn ifu_dstaddr(&mut self) -> *mut nix::sys::socket::sockaddr {
        self.data as *mut nix::sys::socket::sockaddr
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct ifaddrs {
    pub ifa_next: *mut ifaddrs,
    pub ifa_name: *mut std::os::raw::c_char,
    pub ifa_flags: std::os::raw::c_uint,
    pub ifa_addr: *mut nix::sys::socket::sockaddr,
    pub ifa_netmask: *mut nix::sys::socket::sockaddr,
    pub ifa_ifu: union_ifa_ifu,
    pub ifa_data: *mut std::os::raw::c_void,
}

extern "C" {
    pub fn getifaddrs(ifap: *mut *mut ifaddrs) -> std::os::raw::c_int;
    pub fn freeifaddrs(ifa: *mut ifaddrs) -> std::os::raw::c_void;
    #[allow(dead_code)]
    pub fn if_nametoindex(ifname: *const std::os::raw::c_char) -> std::os::raw::c_uint;
}

pub fn nix_socketaddr_to_sockaddr(sa: *mut nix::sys::socket::sockaddr) -> Option<net::SocketAddr> {
    if sa.is_null() {
        return None;
    }

    #[allow(clippy::identity_op)]
    let (addr, port) = match AddressFamily::from_i32(unsafe { *sa }.sa_family as i32)? {
        AF_INET => {
            let sa = sa as *const sockaddr_in;
            let sa = &unsafe { *sa };

            let (addr, port) = (sa.sin_addr.s_addr, sa.sin_port);
            (
                IpAddr::V4(net::Ipv4Addr::new(
                    ((addr & 0x000000FF) >> 0) as u8,
                    ((addr & 0x0000FF00) >> 8) as u8,
                    ((addr & 0x00FF0000) >> 16) as u8,
                    ((addr & 0xFF000000) >> 24) as u8,
                )),
                port,
            )
        }
        AF_INET6 => {
            let sa = sa as *const sockaddr_in6;
            let sa = &unsafe { *sa };
            let (addr, port) = (sa.sin6_addr.s6_addr, sa.sin6_port);
            (
                IpAddr::V6(net::Ipv6Addr::new(
                    addr[0] as u16,
                    addr[1] as u16,
                    addr[2] as u16,
                    addr[3] as u16,
                    addr[4] as u16,
                    addr[5] as u16,
                    addr[6] as u16,
                    addr[7] as u16,
                )),
                port,
            )
        }
        _ => return None,
    };
    Some(net::SocketAddr::new(addr, port))
}

/// Query the local system for all interface addresses.
pub fn ifaces() -> Result<Vec<Interface>, Error> {
    let mut ifaddrs_ptr: *mut ifaddrs = ptr::null_mut();
    match unsafe { getifaddrs(&mut ifaddrs_ptr as *mut _) } {
        0 => {
            let mut ret = Vec::new();
            let mut item: *mut ifaddrs = ifaddrs_ptr;
            loop {
                if item.is_null() {
                    break;
                }
                let name = String::from_utf8(
                    unsafe { CStr::from_ptr((*item).ifa_name) }
                        .to_bytes()
                        .to_vec(),
                );
                unsafe {
                    if name.is_err() || (*item).ifa_addr.is_null() {
                        break;
                    }
                }

                let kind = AddressFamily::from_i32(unsafe { (*(*item).ifa_addr).sa_family } as i32);
                if kind.is_none() {
                    break;
                }
                let kind = match kind.unwrap() {
                    AF_INET => Some(Kind::Ipv4),
                    AF_INET6 => Some(Kind::Ipv6),
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    AF_PACKET => Some(Kind::Packet),
                    #[cfg(any(
                        target_os = "macos",
                        target_os = "ios",
                        target_os = "freebsd",
                        target_os = "openbsd",
                        target_os = "netbsd"
                    ))]
                    AF_LINK => Some(Kind::Link),
                    _ => Some(Kind::Unknown),
                };
                if kind.is_none() {
                    break;
                }

                let addr = nix_socketaddr_to_sockaddr(unsafe { (*item).ifa_addr });
                let mask = nix_socketaddr_to_sockaddr(unsafe { (*item).ifa_netmask });
                let hop = unsafe {
                    if (*item).ifa_flags & SIOCGIFFLAGS::IFF_BROADCAST as std::os::raw::c_uint
                        == SIOCGIFFLAGS::IFF_BROADCAST as std::os::raw::c_uint
                    {
                        nix_socketaddr_to_sockaddr((*item).ifa_ifu.ifu_broadaddr())
                            .map(NextHop::Broadcast)
                    } else {
                        nix_socketaddr_to_sockaddr((*item).ifa_ifu.ifu_dstaddr())
                            .map(NextHop::Destination)
                    }
                };

                if let Some(kind) = kind {
                    if kind != Kind::Unknown {
                        ret.push(Interface {
                            name: name.unwrap(),
                            kind,
                            addr,
                            mask,
                            hop,
                        });
                    }
                };

                item = unsafe { (*item).ifa_next };
            }
            unsafe { freeifaddrs(ifaddrs_ptr) };
            Ok(ret)
        }
        _ => Err(Error::new(ErrorKind::Other, "Oh, no ...")), // Err(nix::errno::Errno::last());
    }
}
