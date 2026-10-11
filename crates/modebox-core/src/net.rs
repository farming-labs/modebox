//! Network access by host name.
//!
//! [`HostAllow`] decides which hosts sandboxed code may reach, and [`connect`]
//! resolves and connects in one step, so the address that was checked is the
//! one connected to. [`Proxy`] puts both behind an HTTP proxy, which is how
//! the shell sandbox gets network access to some hosts and no others.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// Host patterns: `example.com` (that host), `*.example.com` (its
/// subdomains), `*` (any host). An exact name may resolve to a private or
/// loopback address; a wildcard only reaches public addresses, so `*` can't
/// be used to probe the local network.
#[derive(Clone, Debug, Default)]
pub struct HostAllow {
    patterns: Vec<String>,
}

impl HostAllow {
    pub fn new<S: AsRef<str>>(patterns: impl IntoIterator<Item = S>) -> HostAllow {
        HostAllow { patterns: patterns.into_iter().map(|p| normalize(p.as_ref())).filter(|p| !p.is_empty()).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// `None` if `host` isn't allowed; otherwise whether private addresses are.
    pub fn check(&self, host: &str) -> Option<bool> {
        let host = normalize(host);
        let mut wildcard = false;
        for p in &self.patterns {
            if *p == host {
                return Some(true);
            }
            if p == "*" || p.strip_prefix('*').is_some_and(|suffix| suffix.starts_with('.') && host.ends_with(suffix)) {
                wildcard = true;
            }
        }
        wildcard.then_some(false)
    }
}

fn normalize(host: &str) -> String {
    host.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase()
}

/// Whether an address is on the public internet (not loopback, private,
/// link-local, shared, documentation, multicast or unspecified).
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_v4(v4),
            None => public_v6(v6),
        },
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || a >= 240)
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link-local
        || (s[0] == 0x2001 && s[1] == 0x0db8)) // documentation
}

/// Connect to `host:port` if `allow` permits it. Resolution happens here, and
/// every address is checked before it's connected to, so DNS can't swap in a
/// private address after the check.
pub async fn connect(allow: &HostAllow, host: &str, port: u16) -> Result<TcpStream, String> {
    let host = normalize(host);
    let private_ok = allow.check(&host).ok_or_else(|| format!("{host} isn't on the network allow list"))?;
    let addrs = tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| format!("{host}: {e}"))?;
    let mut why = format!("{host}: no addresses");
    for addr in addrs {
        if !private_ok && !is_public(addr.ip()) {
            why = format!("{host} resolves to a non-public address ({}); list it exactly to allow that", addr.ip());
            continue;
        }
        match TcpStream::connect(addr).await {
            Ok(s) => return Ok(s),
            Err(e) => why = format!("{host}: {e}"),
        }
    }
    Err(why)
}

/// Where a running [`Proxy`] listens.
#[derive(Clone, Debug)]
pub enum ProxyAddr {
    /// `127.0.0.1:port`.
    Tcp(u16),
    /// A Unix socket.
    Unix(PathBuf),
}

/// An HTTP proxy that only reaches allowed hosts: `CONNECT host:port` for
/// HTTPS, and absolute-form requests (`GET http://host/path`) for HTTP.
/// Anything else, or any other host, gets 403. It runs until dropped.
pub struct Proxy {
    pub addr: ProxyAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
        if let ProxyAddr::Unix(p) = &self.addr {
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Proxy {
    /// Listen on an ephemeral loopback port.
    pub async fn tcp(allow: HostAllow) -> std::io::Result<Proxy> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let allow = Arc::new(allow);
        let task = tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                tokio::spawn(serve(conn, allow.clone()));
            }
        });
        Ok(Proxy { addr: ProxyAddr::Tcp(port), task })
    }

    /// Listen on a Unix socket at `path`.
    #[cfg(unix)]
    pub async fn unix(allow: HostAllow, path: &Path) -> std::io::Result<Proxy> {
        let _ = std::fs::remove_file(path);
        let listener = tokio::net::UnixListener::bind(path)?;
        let allow = Arc::new(allow);
        let task = tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                tokio::spawn(serve(conn, allow.clone()));
            }
        });
        Ok(Proxy { addr: ProxyAddr::Unix(path.to_owned()), task })
    }
}

/// Longest request head the proxy reads before giving up.
const MAX_HEAD: usize = 16 << 10;

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(mut client: S, allow: Arc<HostAllow>) {
    let mut buf = Vec::with_capacity(1024);
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            return refuse(&mut client, 431, "request head too large").await;
        }
        match client.read_buf(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let rest = buf[head_end..].to_vec();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or("HTTP/1.1"));

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_host_port(target, 443) else {
            return refuse(&mut client, 400, "bad CONNECT target").await;
        };
        let mut upstream = match connect(&allow, &host, port).await {
            Ok(s) => s,
            Err(why) => return refuse(&mut client, 403, &why).await,
        };
        if client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err()
            || upstream.write_all(&rest).await.is_err()
        {
            return;
        }
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        return;
    }

    // Plain HTTP: forward the request with its origin-form target.
    let Some(after) = target.strip_prefix("http://") else {
        return refuse(&mut client, 400, "only CONNECT and http:// requests go through this proxy").await;
    };
    let (authority, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, "/"),
    };
    let Some((host, port)) = split_host_port(authority, 80) else {
        return refuse(&mut client, 400, "bad request target").await;
    };
    let mut upstream = match connect(&allow, &host, port).await {
        Ok(s) => s,
        Err(why) => return refuse(&mut client, 403, &why).await,
    };
    let mut out = format!("{method} {path} {version}\r\n");
    for line in lines.filter(|l| !l.is_empty()) {
        let name = line.split(':').next().unwrap_or("").trim();
        if !name.eq_ignore_ascii_case("proxy-connection") && !name.eq_ignore_ascii_case("proxy-authorization") {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("\r\n");
    if upstream.write_all(out.as_bytes()).await.is_err() || upstream.write_all(&rest).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

async fn refuse<S: AsyncWrite + Unpin>(client: &mut S, status: u16, why: &str) {
    let reason = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        _ => "Request Header Fields Too Large",
    };
    let body = format!("modebox: {why}\n");
    let msg = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = client.write_all(msg.as_bytes()).await;
}

/// `host:port`, `host` (default port) or `[v6]:port`.
fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => default_port,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Some((host.to_owned(), port.parse().ok()?)),
        Some(_) => None,
        None if !s.is_empty() => Some((s.to_owned(), default_port)),
        None => None,
    }
}

/// Port the Linux network bridge listens on inside the sandbox.
pub const BRIDGE_PORT: u16 = 3128;

/// The Linux network bridge: run inside the sandbox's isolated network as
/// `<program> __net-bridge <socket> -- <command...>`. It listens on
/// `127.0.0.1:3128` there, forwards each connection to the host's proxy
/// socket, runs the command, and exits with its status. A binary embedding
/// modebox calls this when it sees those arguments (the `modebox` CLI does).
#[cfg(unix)]
pub fn net_bridge_main(args: &[String]) -> ! {
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;
    let fail = |msg: &str| -> ! {
        let _ = writeln!(std::io::stderr(), "modebox network bridge: {msg}");
        std::process::exit(126)
    };
    let (socket, command) = match args {
        [socket, sep, command @ ..] if sep == "--" && !command.is_empty() => (PathBuf::from(socket), command),
        _ => fail("usage: __net-bridge <socket> -- <command...>"),
    };
    let listener = std::net::TcpListener::bind(("127.0.0.1", BRIDGE_PORT)).unwrap_or_else(|e| fail(&e.to_string()));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(tcp) = conn else { continue };
            let socket = socket.clone();
            std::thread::spawn(move || {
                let Ok(unix) = UnixStream::connect(&socket) else { return };
                let (Ok(mut tcp_r), Ok(mut unix_r)) = (tcp.try_clone(), unix.try_clone()) else { return };
                let (mut tcp_w, mut unix_w) = (tcp, unix);
                let up = std::thread::spawn(move || {
                    let _ = std::io::copy(&mut tcp_r, &mut unix_w);
                    let _ = unix_w.shutdown(std::net::Shutdown::Write);
                });
                let _ = std::io::copy(&mut unix_r, &mut tcp_w);
                let _ = tcp_w.shutdown(std::net::Shutdown::Write);
                let _ = up.join();
            });
        }
    });
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .status()
        .unwrap_or_else(|e| fail(&format!("{}: {e}", command[0])));
    std::process::exit(status.code().unwrap_or(128 + 9))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_patterns() {
        let a = HostAllow::new(["Example.com", "*.npmjs.org", "10.0.0.5"]);
        assert_eq!(a.check("example.com."), Some(true));
        assert_eq!(a.check("www.example.com"), None);
        assert_eq!(a.check("registry.npmjs.org"), Some(false));
        assert_eq!(a.check("npmjs.org"), None);
        assert_eq!(a.check("evilnpmjs.org"), None);
        assert_eq!(a.check("10.0.0.5"), Some(true));
        assert_eq!(HostAllow::new(["*"]).check("anything.test"), Some(false));
        assert_eq!(HostAllow::new(["[::1]"]).check("::1"), Some(true));
    }

    #[test]
    fn public_addresses() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1"] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "1.1.1.1", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn host_ports() {
        assert_eq!(split_host_port("a.b:8443", 443), Some(("a.b".into(), 8443)));
        assert_eq!(split_host_port("a.b", 80), Some(("a.b".into(), 80)));
        assert_eq!(split_host_port("[::1]:9", 443), Some(("::1".into(), 9)));
        assert_eq!(split_host_port("::1", 443), None);
        assert_eq!(split_host_port("a.b:x", 443), None);
    }

    #[tokio::test]
    async fn the_proxy_refuses_hosts_off_the_list() {
        let proxy = Proxy::tcp(HostAllow::new(["allowed.test"])).await.unwrap();
        let ProxyAddr::Tcp(port) = proxy.addr else { unreachable!() };
        let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(b"CONNECT evil.test:443 HTTP/1.1\r\nHost: evil.test\r\n\r\n").await.unwrap();
        let mut reply = String::new();
        c.read_to_string(&mut reply).await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 403") && reply.contains("isn't on the network allow list"), "{reply}");
    }

    #[tokio::test]
    async fn wildcards_dont_reach_private_addresses() {
        let err = connect(&HostAllow::new(["*"]), "127.0.0.1", 9).await.unwrap_err();
        assert!(err.contains("non-public"), "{err}");
    }
}
