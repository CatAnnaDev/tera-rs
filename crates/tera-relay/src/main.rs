use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(about = "Relais TCP transparent pour TERA: egress direct ou via SOCKS5")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:19003")]
    listen: String,
    #[arg(long, default_value = "155.103.80.244:19003")]
    target: String,
    #[arg(long)]
    socks5: Option<String>,
    #[arg(long, help = "utilisateur SOCKS5 (proxy avec auth, ex: residentiel)")]
    socks5_user: Option<String>,
    #[arg(long, help = "mot de passe SOCKS5")]
    socks5_pass: Option<String>,
    #[arg(long, default_value_t = 15)]
    connect_timeout: u64,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        help = "pose TCP_NOOPT sur l'egress (macOS): SYN sans option TCP, donc sans timestamp (passe la couche 1)"
    )]
    noopt: bool,
}

static CONN_SEQ: AtomicU64 = AtomicU64::new(0);

fn main() -> Result<()> {
    let cli = Arc::new(Cli::parse());
    let listener = TcpListener::bind(&cli.listen)
        .with_context(|| format!("bind {}", cli.listen))?;
    let via = cli.socks5.as_deref().unwrap_or("direct");
    let noopt = if cli.noopt && cli.socks5.is_none() { "TCP_NOOPT on" } else { "TCP_NOOPT off" };
    println!(
        "[relay] listen {} -> target {} (egress: {}, {})",
        cli.listen, cli.target, via, noopt
    );

    for stream in listener.incoming() {
        match stream {
            Ok(client) => {
                let cli = Arc::clone(&cli);
                std::thread::spawn(move || {
                    let id = CONN_SEQ.fetch_add(1, Ordering::Relaxed);
                    if let Err(e) = handle(id, client, &cli) {
                        eprintln!("[relay #{id}] erreur: {e:#}");
                    }
                });
            }
            Err(e) => eprintln!("[relay] accept: {e}"),
        }
    }
    Ok(())
}

fn handle(id: u64, client: TcpStream, cli: &Cli) -> Result<()> {
    let t0 = Instant::now();
    let peer = client
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    println!("[relay #{id}] client {peer} connecte");

    let timeout = Duration::from_secs(cli.connect_timeout);
    let upstream = match &cli.socks5 {
        Some(proxy) => socks5_connect(
            proxy,
            &cli.target,
            timeout,
            cli.socks5_user.as_deref(),
            cli.socks5_pass.as_deref(),
        )
        .with_context(|| format!("SOCKS5 via {proxy}"))?,
        None => connect_direct(&cli.target, timeout, cli.noopt)
            .with_context(|| format!("connexion directe {}", cli.target))?,
    };
    println!(
        "[relay #{id}] upstream {} etabli en {:.3}s",
        cli.target,
        t0.elapsed().as_secs_f64()
    );

    client.set_nodelay(true).ok();
    upstream.set_nodelay(true).ok();

    let c2s_client = client.try_clone().context("clone client")?;
    let c2s_up = upstream.try_clone().context("clone upstream")?;

    let up_bytes = Arc::new(AtomicU64::new(0));
    let down_bytes = Arc::new(AtomicU64::new(0));

    let up_counter = Arc::clone(&up_bytes);
    let c2s = std::thread::spawn(move || pump(c2s_client, c2s_up, &up_counter));

    let down_counter = Arc::clone(&down_bytes);
    pump(upstream, client, &down_counter);
    let _ = c2s.join();

    println!(
        "[relay #{id}] termine apres {:.3}s | client->serveur {} o | serveur->client {} o",
        t0.elapsed().as_secs_f64(),
        up_bytes.load(Ordering::Relaxed),
        down_bytes.load(Ordering::Relaxed)
    );
    Ok(())
}

fn pump(mut from: TcpStream, mut to: TcpStream, counter: &AtomicU64) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                counter.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = to.shutdown(Shutdown::Write);
    let _ = from.shutdown(Shutdown::Read);
}

fn resolve(addr: &str) -> Result<std::net::SocketAddr> {
    addr.to_socket_addrs()
        .with_context(|| format!("resolution {addr}"))?
        .next()
        .ok_or_else(|| anyhow!("aucune adresse pour {addr}"))
}

fn connect_timeout(addr: &str, timeout: Duration) -> Result<TcpStream> {
    let sa = resolve(addr)?;
    let stream = TcpStream::connect_timeout(&sa, timeout)?;
    Ok(stream)
}

fn connect_direct(addr: &str, timeout: Duration, noopt: bool) -> Result<TcpStream> {
    let sa = resolve(addr)?;
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(sa),
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    if noopt {
        set_tcp_noopt(&socket)?;
    }
    socket.connect_timeout(&socket2::SockAddr::from(sa), timeout)?;
    Ok(TcpStream::from(socket))
}

#[cfg(target_os = "macos")]
fn set_tcp_noopt(socket: &socket2::Socket) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    const TCP_NOOPT: libc::c_int = 0x08;
    let enable: libc::c_int = 1;
    let ret = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            TCP_NOOPT,
            &enable as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        bail!("setsockopt TCP_NOOPT: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn set_tcp_noopt(_socket: &socket2::Socket) -> Result<()> {
    Ok(())
}

fn socks5_connect(
    proxy: &str,
    target: &str,
    timeout: Duration,
    user: Option<&str>,
    pass: Option<&str>,
) -> Result<TcpStream> {
    let mut s = connect_timeout(proxy, timeout)?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();

    let auth = user.is_some() || pass.is_some();
    if auth {
        s.write_all(&[0x05, 0x02, 0x00, 0x02])?;
    } else {
        s.write_all(&[0x05, 0x01, 0x00])?;
    }
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello)?;
    if hello[0] != 0x05 {
        bail!("reponse SOCKS5 invalide: {:02x} {:02x}", hello[0], hello[1]);
    }
    match hello[1] {
        0x00 => {}
        0x02 => {
            let u = user.unwrap_or("").as_bytes();
            let p = pass.unwrap_or("").as_bytes();
            if u.len() > 255 || p.len() > 255 {
                bail!("identifiants SOCKS5 trop longs");
            }
            let mut req = Vec::with_capacity(3 + u.len() + p.len());
            req.push(0x01);
            req.push(u.len() as u8);
            req.extend_from_slice(u);
            req.push(p.len() as u8);
            req.extend_from_slice(p);
            s.write_all(&req)?;
            let mut reply = [0u8; 2];
            s.read_exact(&mut reply)?;
            if reply[1] != 0x00 {
                bail!("auth SOCKS5 refusee (status {:#04x})", reply[1]);
            }
        }
        0xff => bail!("SOCKS5: aucune methode d'auth acceptee (identifiants requis ?)"),
        other => bail!("SOCKS5: methode d'auth inattendue {other:#04x}"),
    }

    let (host, port) = split_hostport(target)?;
    let mut req = Vec::with_capacity(7 + host.len());
    req.extend_from_slice(&[0x05, 0x01, 0x00]);
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(v4) => {
            req.push(0x01);
            req.extend_from_slice(&v4.octets());
        }
        Err(_) => {
            let hb = host.as_bytes();
            if hb.len() > 255 {
                bail!("hostname trop long");
            }
            req.push(0x03);
            req.push(hb.len() as u8);
            req.extend_from_slice(hb);
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req)?;

    let mut head = [0u8; 4];
    s.read_exact(&mut head)?;
    if head[1] != 0x00 {
        bail!("SOCKS5 CONNECT echoue, code {:#04x}", head[1]);
    }
    let skip = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l)?;
            l[0] as usize
        }
        other => bail!("ATYP inconnu {other:#04x}"),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest)?;

    s.set_read_timeout(None).ok();
    s.set_write_timeout(None).ok();
    Ok(s)
}

fn split_hostport(target: &str) -> Result<(String, u16)> {
    let (h, p) = target
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("cible sans port: {target}"))?;
    let port: u16 = p.parse().with_context(|| format!("port invalide: {p}"))?;
    Ok((h.to_string(), port))
}
