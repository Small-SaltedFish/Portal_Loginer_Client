use crate::transport::TransportError;
use hickory_resolver::proto::{
    op::{Message, MessageType, Query},
    rr::{Name as DnsName, RData, RecordType},
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
pub struct BoundDns {
    pub source: Ipv4Addr,
    pub servers: Vec<Ipv4Addr>,
}
#[cfg(windows)]
fn pin(socket: &std::net::UdpSocket, source: Ipv4Addr) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    #[link(name = "Ws2_32")]
    extern "system" {
        fn setsockopt(socket: usize, level: i32, option: i32, value: *const u8, len: i32) -> i32;
        fn WSAGetLastError() -> i32;
    }
    let index = source_interface(source)?;
    let index = index.to_be();
    if unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            0,
            31,
            (&index as *const u32).cast(),
            4,
        )
    } != 0
    {
        return Err(std::io::Error::from_raw_os_error(unsafe {
            WSAGetLastError()
        }));
    }
    Ok(())
}
impl Resolve for BoundDns {
    fn resolve(&self, name: Name) -> Resolving {
        let source = self.source;
        let servers = self.servers.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let query = Query::query(DnsName::from_ascii(&host)?, RecordType::A);
            let mut message = Message::new();
            message
                .set_id(rand::random())
                .set_recursion_desired(true)
                .add_query(query.clone());
            let bytes = message.to_vec()?;
            for server in servers {
                let attempt = async {
                    let socket = std::net::UdpSocket::bind((source, 0))?;
                    #[cfg(windows)]
                    pin(&socket, source)?;
                    socket.set_nonblocking(true)?;
                    let socket = tokio::net::UdpSocket::from_std(socket)?;
                    socket.connect((server, 53)).await?;
                    socket.send(&bytes).await?;
                    let mut buffer = [0u8; 4096];
                    let size = socket.recv(&mut buffer).await?;
                    let response =
                        Message::from_vec(&buffer[..size]).map_err(std::io::Error::other)?;
                    if response.id() != message.id()
                        || response.message_type() != MessageType::Response
                        || response.queries() != [query.clone()]
                    {
                        return Err(std::io::Error::other("DNS response mismatch"));
                    }
                    let answers: Vec<_> = response
                        .answers()
                        .iter()
                        .filter_map(|r| match r.data() {
                            Some(RData::A(ip)) if super::transport::usable_dns_answer(&ip.0) => {
                                Some(SocketAddr::from((ip.0, 0)))
                            }
                            _ => None,
                        })
                        .collect();
                    Ok::<_, std::io::Error>(answers)
                };
                if let Ok(Ok(answers)) = tokio::time::timeout(Duration::from_secs(2), attempt).await
                {
                    if !answers.is_empty() {
                        return Ok(Box::new(answers.into_iter()) as Addrs);
                    }
                }
            }
            Err(std::io::Error::other(TransportError::Dns).into())
        })
    }
}

#[cfg(windows)]
fn source_interface(source: std::net::Ipv4Addr) -> std::io::Result<u32> {
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetIpAddrTable(table: *mut std::ffi::c_void, size: *mut u32, order: i32) -> u32;
    }
    let mut size = 0u32;
    unsafe {
        GetIpAddrTable(std::ptr::null_mut(), &mut size, 0);
    }
    if !(4..=1024 * 1024).contains(&size) {
        return Err(std::io::Error::other("invalid address table size"));
    }
    let mut table = vec![0u32; (size as usize).div_ceil(4)];
    let result = unsafe { GetIpAddrTable(table.as_mut_ptr().cast(), &mut size, 0) };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    for row in table[1..].chunks_exact(6).take(table[0] as usize) {
        if row[0].to_ne_bytes() == source.octets() {
            return Ok(row[1]);
        }
    }
    Err(std::io::Error::other("selected interface disappeared"))
}
