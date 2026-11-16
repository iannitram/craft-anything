//! Real requests through the production download configuration and file transaction.
use super::*;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

const PAYLOAD: &[u8] = b"engine archive fixture";
const SIGNATURE: &str = "signed-query-must-stay-private";

struct Server {
    url: String,
    seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    fn start(tls: Option<Arc<rustls::ServerConfig>>, reply: impl Fn(&str) -> String + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("{}://localhost:{}", if tls.is_some() { "https" } else { "http" }, listener.local_addr().unwrap().port());
        let seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, done) = (seen.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        socket.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
                        if let Some(config) = &tls {
                            let conn = rustls::ServerConnection::new(config.clone()).unwrap();
                            respond(&mut rustls::StreamOwned::new(conn, socket), &count, &reply);
                        } else {
                            respond(&mut { socket }, &count, &reply);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
        });
        Self { url, seen, stop, worker: Some(worker) }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn respond(stream: &mut (impl Read + Write), count: &AtomicUsize, reply: &impl Fn(&str) -> String) {
    let mut header = Vec::new();
    let mut byte = [0];
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while header.len() < 8192 && !header.ends_with(b"\r\n\r\n") && std::time::Instant::now() < deadline {
        match stream.read(&mut byte) {
            Ok(1) => header.push(byte[0]),
            _ => return,
        }
    }
    if !header.ends_with(b"\r\n\r\n") {
        return;
    }
    count.fetch_add(1, Ordering::SeqCst);
    let response = reply(&String::from_utf8_lossy(&header));
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn redirect(url: &str) -> String {
    format!("HTTP/1.1 302 Found\r\nLocation: {url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

fn payload(_: &str) -> String {
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", PAYLOAD.len(), String::from_utf8_lossy(PAYLOAD))
}

struct Tls {
    server: Arc<rustls::ServerConfig>,
    agent: ureq::Agent,
}

impl Tls {
    fn new() -> Self {
        let key = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert = key.cert.der().clone();
        let private = rustls::pki_types::PrivatePkcs8KeyDer::from(key.signing_key.serialize_der());
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], private.into())
            .unwrap();
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::new_with_certs(&[ureq::tls::Certificate::from_der(cert.as_ref()).to_owned()]))
            .build();
        // Only this client trusts this runtime-generated certificate. No global TLS/proxy changes.
        let agent = download_config()
            .proxy(None)
            .tls_config(tls)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_recv_response(Some(Duration::from_secs(3)))
            .timeout_recv_body(Some(Duration::from_secs(3)))
            .build()
            .new_agent();
        Self { server: Arc::new(server), agent }
    }

    fn save(&self, url: &str, dest: &Path, hash: &str) -> Result<(), ToolError> {
        write_checked(dest, Some(hash), url, &mut |sink| download_with_agent(url, sink, 4096, None, &|_, _| {}, &self.agent))
    }
}

#[test]
fn initial_http_is_rejected_without_a_request() {
    let http = Server::start(None, payload);
    let error = download_to(&http.url, &mut Vec::new(), 4096, None, &|_, _| {}).unwrap_err();
    assert!(error.message.contains("https"));
    assert_eq!(http.seen.load(Ordering::SeqCst), 0);
}

#[test]
fn downgrade_never_reaches_http_or_replaces_a_file_or_discloses_the_link() {
    let tls = Tls::new();
    let http = Server::start(None, payload);
    let target = format!("{}/archive?signature={SIGNATURE}", http.url);
    let https = Server::start(Some(tls.server.clone()), move |_| redirect(&target));
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("engine.zip");
    std::fs::write(&dest, b"old archive").unwrap();
    let result = tls.save(&format!("{}?token={SIGNATURE}", https.url), &dest, &hex::encode(Sha256::digest(PAYLOAD)));
    assert_eq!(http.seen.load(Ordering::SeqCst), 0);
    let error = result.unwrap_err();
    assert_eq!(https.seen.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&dest).unwrap(), b"old archive");
    assert!(!dest.with_extension("download").exists());
    assert!(!format!("{error:?}").contains(SIGNATURE));
    assert!(error.message.contains("HTTP"));
}

#[test]
fn https_redirect_downloads_and_verifies_the_payload() {
    let tls = Tls::new();
    let final_host = Server::start(Some(tls.server.clone()), payload);
    let target = final_host.url.clone();
    let first = Server::start(Some(tls.server.clone()), move |_| redirect(&target));
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("engine.zip");
    tls.save(&first.url, &dest, &hex::encode(Sha256::digest(PAYLOAD))).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), PAYLOAD);
    assert_eq!(final_host.seen.load(Ordering::SeqCst), 1);
    assert_eq!(first.seen.load(Ordering::SeqCst), 1);
    assert!(!dest.with_extension("download").exists());
}

#[test]
fn checksum_failure_keeps_the_old_file() {
    let tls = Tls::new();
    let https = Server::start(Some(tls.server.clone()), payload);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("engine.zip");
    std::fs::write(&dest, b"old").unwrap();
    let error = tls.save(&https.url, &dest, &"0".repeat(64)).unwrap_err();
    assert_eq!(error.code, ErrorCode::FileHashMismatch);
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    assert!(!dest.with_extension("download").exists());
}

#[test]
fn redirect_cycles_stop_at_the_existing_limit() {
    let tls = Tls::new();
    let https = Server::start(Some(tls.server.clone()), |_| redirect("/again"));
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("engine.zip");
    assert!(tls.save(&https.url, &dest, &hex::encode(Sha256::digest(PAYLOAD))).is_err());
    assert_eq!(https.seen.load(Ordering::SeqCst), 11);
    assert!(!dest.exists() && !dest.with_extension("download").exists());
}

#[test]
fn an_untrusted_certificate_is_still_refused() {
    let tls = Tls::new();
    let https = Server::start(Some(tls.server.clone()), payload);
    let agent = download_config().proxy(None).build().new_agent();
    assert!(download_with_agent(&https.url, &mut Vec::new(), 4096, None, &|_, _| {}, &agent).is_err());
    assert_eq!(https.seen.load(Ordering::SeqCst), 0);
}

/// Windows' manual proxy setting, as the registry keeps it (#1827).
#[test]
fn a_manual_system_proxy_is_read_as_windows_writes_it() {
    let uri = |s: &str| s.parse::<ureq::http::Uri>().unwrap();
    let addr = |p: ureq::Proxy| format!("{}:{}", p.host(), p.port());
    assert_eq!(addr(manual_proxy("127.0.0.1:10808", "").unwrap()), "127.0.0.1:10808");
    assert_eq!(addr(manual_proxy("http=h1:80;https=h2:8443;socks=s:1080", "").unwrap()), "h2:8443", "the HTTPS entry");
    assert_eq!(addr(manual_proxy("ftp=f:21;http=h1:80", "").unwrap()), "h1:80", "else the HTTP one");
    assert_eq!(addr(manual_proxy("[::1]:3128", "").unwrap()), "[::1]:3128");
    for refused in ["", "socks=s:1080", "https=bad;http=h1:80", "user:pw@h:80", "h", "fe80::1", "http://h:80/path"] {
        assert!(manual_proxy(refused, "").is_none(), "{refused}");
    }

    let p = manual_proxy("proxy:8080", "*.corp.example;intranet;<local>").unwrap();
    for direct in ["https://localhost/x", "https://127.0.0.1/x", "https://[::1]/x", "https://a.corp.example/x", "https://intranet/x"] {
        assert!(p.is_no_proxy(&uri(direct)), "{direct} goes direct");
    }
    assert!(!p.is_no_proxy(&uri("https://github.com/x")), "a download goes through the proxy");

    // Rules ureq can't match as Windows does are left out, not read as something else.
    let p = manual_proxy("proxy:8080", "*github*;10.*.0.1;github.*").unwrap();
    assert!(!p.is_no_proxy(&uri("https://api.github.com/x")) && !p.is_no_proxy(&uri("https://10.1.0.1/x")));
    assert!(p.is_no_proxy(&uri("https://github.com/x")), "a single `*` at the end still works");
}
