//! Protocol tests against a fake bridge server (a thread that speaks our NDJSON protocol). No
//! Ghidra is involved: these prove framing, token auth, request timeouts, cancellation and that
//! dropping the client cleans up the connection.

use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How the fake server should behave once it has a client.
#[derive(Clone, Copy)]
enum Mode {
    /// Answer requests normally (token must equal `expected`).
    Serve,
    /// Accept the connection and then stay silent forever.
    Silent,
    /// Answer the first request with a reply line far longer than a test client accepts.
    Huge,
    /// Answer the first request with a reply sent in two halves, a few read ticks apart.
    Split,
}

struct Fake {
    addr: SocketAddr,
    saw_eof: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Fake {
    fn start(mode: Mode, expected_token: &'static str) -> Fake {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let saw_eof = Arc::new(AtomicBool::new(false));
        let eof2 = saw_eof.clone();
        let handle = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                match mode {
                    Mode::Silent => {
                        // Hold the connection open; read so we notice the client's EOF.
                        let mut reader = BufReader::new(stream);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            match reader.read_line(&mut line) {
                                Ok(0) => {
                                    eof2.store(true, Ordering::SeqCst);
                                    break;
                                }
                                Ok(_) => { /* ignore; never reply */ }
                                Err(_) => break,
                            }
                        }
                    }
                    Mode::Serve => serve(stream, expected_token, &eof2),
                    Mode::Huge | Mode::Split => {
                        let mut writer = stream.try_clone().expect("clone");
                        let mut reader = BufReader::new(stream);
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let id = serde_json::from_str::<Request>(line.trim()).map(|r| r.id).unwrap_or(1);
                        let names = "f".repeat(64 * 1024);
                        let reply = format!("{{\"id\":{id},\"ok\":true,\"result\":{{\"functions\":[{{\"name\":\"{names}\",\"address\":\"0x1\"}}]}}}}\n");
                        if matches!(mode, Mode::Split) {
                            let (a, b) = reply.as_bytes().split_at(reply.len() / 2);
                            let _ = writer.write_all(a);
                            let _ = writer.flush();
                            std::thread::sleep(Duration::from_millis(600));
                            let _ = writer.write_all(b);
                        } else {
                            let _ = writer.write_all(reply.as_bytes());
                        }
                        let _ = writer.flush();
                        // Wait for the client to go away.
                        line.clear();
                        while reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                            line.clear();
                        }
                        eof2.store(true, Ordering::SeqCst);
                    }
                }
            }
        });
        Fake { addr, saw_eof, handle: Some(handle) }
    }

    fn join(mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(stream: TcpStream, expected: &str, saw_eof: &Arc<AtomicBool>) {
    let mut writer = stream.try_clone().expect("clone");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                saw_eof.store(true, Ordering::SeqCst);
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => {
                let _ = writer.write_all(b"{\"id\":1,\"ok\":false,\"error\":{\"code\":\"bad_input\",\"message\":\"bad json\"}}\n");
                break;
            }
        };
        // Constant-time token check, as the real bridge does.
        if !ct_eq(req.token.as_bytes(), expected.as_bytes()) {
            let resp = format!("{{\"id\":{},\"ok\":false,\"error\":{{\"code\":\"bad_input\",\"message\":\"bad token\"}}}}\n", req.id);
            let _ = writer.write_all(resp.as_bytes());
            break; // close on a bad token
        }
        let result = match req.method.as_str() {
            "ping" => "{\"name\":\"fake\",\"provider\":{\"id\":\"ghidra\",\"version\":\"12.1.4\"},\"analysed\":true,\"analysis_cut_short\":false,\"target\":{\"name\":\"t\",\"image_base\":\"0x100000\"}}"
                .to_string(),
            "native_functions_list" => "{\"functions\":[{\"name\":\"main\",\"address\":\"0x401000\"}]}".to_string(),
            // As the real bridge: a known function, code its analysis never made a function (made
            // on demand), and an address outside executable memory (refused with the reason).
            "native_function_pseudocode" => match req.params.get("address").and_then(Value::as_str) {
                Some("0x401200") => "{\"name\":\"FUN_00401200\",\"address\":\"0x401200\",\"signature\":\"int FUN_00401200(int)\",\"pseudocode\":\"int FUN_00401200(int a){return a*3;}\",\"created\":true}".to_string(),
                Some("0x600000") => {
                    let _ = writer.write_all(format!("{{\"id\":{},\"ok\":false,\"error\":{{\"code\":\"not_found\",\"message\":\"the address is not in executable memory\"}}}}
", req.id).as_bytes());
                    continue;
                }
                _ => "{\"name\":\"main\",\"address\":\"0x401000\",\"signature\":\"int main(void)\",\"pseudocode\":\"int main(void){return 0;}\",\"created\":false}".to_string(),
            },
            "shutdown" => {
                let _ = writer.write_all(format!("{{\"id\":{},\"ok\":true,\"result\":{{\"shutdown\":true}}}}\n", req.id).as_bytes());
                break;
            }
            _ => {
                let _ = writer.write_all(format!("{{\"id\":{},\"ok\":false,\"error\":{{\"code\":\"not_found\",\"message\":\"no\"}}}}\n", req.id).as_bytes());
                continue;
            }
        };
        let _ = writer.write_all(format!("{{\"id\":{},\"ok\":true,\"result\":{result}}}\n", req.id).as_bytes());
        let _ = writer.flush();
    }
}

fn no_cancel() -> impl Fn() -> bool {
    || false
}

#[test]
fn framing_ping_and_methods() {
    let fake = Fake::start(Mode::Serve, "secret-token");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = Client::connect(fake.addr, "secret-token", deadline).expect("connect");
    let info = client.ping(&no_cancel(), deadline).expect("ping");
    assert_eq!(info["provider"]["id"], "ghidra");
    assert_eq!(image_base_of(&info), Some(0x10_0000), "the handshake says where Ghidra loaded the program");
    assert_eq!(image_base_of(&serde_json::json!({"target": {"name": "t"}})), None, "an older bridge doesn't say");

    let list = client.request("native_functions_list", serde_json::json!({}), &no_cancel(), deadline).expect("list");
    assert_eq!(list["functions"][0]["name"], "main");

    let dec = client.request("native_function_pseudocode", serde_json::json!({"address": "0x401000"}), &no_cancel(), deadline).expect("decompile");
    assert!(dec["pseudocode"].as_str().unwrap().contains("return 0"));

    let unknown = client.request("does_not_exist", serde_json::json!({}), &no_cancel(), deadline).unwrap_err();
    assert!(matches!(unknown, GhidraError::Bridge { .. }));

    client.shutdown();
    fake.join();
}

#[test]
fn code_without_a_function_is_made_on_demand_and_data_is_refused_with_the_reason() {
    let fake = Fake::start(Mode::Serve, "tok");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = Client::connect(fake.addr, "tok", deadline).expect("connect");
    let mut ask = |addr: &str| client.request("native_function_pseudocode", serde_json::json!({ "address": addr }), &no_cancel(), deadline);

    let known = decompiled(0x401000, &ask("0x401000").expect("known")).expect("an answer");
    assert!(!known.created);
    let made = decompiled(0x401200, &ask("0x401200").expect("made")).expect("an answer");
    assert!(made.created && made.pseudocode.as_deref().unwrap_or_default().contains("a*3"), "{made:?}");
    match ask("0x600000").unwrap_err() {
        GhidraError::Bridge { code, message } => assert!(code == "not_found" && message.contains("executable memory"), "{code}: {message}"),
        other => panic!("expected not_found, got {other:?}"),
    }
    assert!(decompiled(0x1, &Value::Null).is_none(), "null means no answer");
    client.shutdown();
    fake.join();
}

#[test]
fn bad_token_is_rejected() {
    let fake = Fake::start(Mode::Serve, "the-real-token");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = Client::connect(fake.addr, "the-wrong-token", deadline).expect("connect");
    let err = client.ping(&no_cancel(), deadline).unwrap_err();
    match err {
        GhidraError::Bridge { code, .. } => assert_eq!(code, "bad_input"),
        other => panic!("expected a bridge rejection, got {other:?}"),
    }
    fake.join();
}

#[test]
fn request_times_out_without_hanging() {
    let fake = Fake::start(Mode::Silent, "tok");
    let mut client = Client::connect(fake.addr, "tok", Instant::now() + Duration::from_secs(5)).expect("connect");
    let t = Instant::now();
    let err = client.request("ping", serde_json::json!({}), &no_cancel(), Instant::now() + Duration::from_millis(400)).unwrap_err();
    assert!(matches!(err, GhidraError::Timeout(_)), "{err:?}");
    assert!(t.elapsed() < Duration::from_secs(3), "took {:?}", t.elapsed());
    drop(client);
    fake.join();
}

#[test]
fn cancel_stops_a_request_quickly() {
    let fake = Fake::start(Mode::Silent, "tok");
    let mut client = Client::connect(fake.addr, "tok", Instant::now() + Duration::from_secs(5)).expect("connect");
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    // Flip cancel shortly after the call starts.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        f2.store(true, Ordering::SeqCst);
    });
    let t = Instant::now();
    let err = client.request("ping", serde_json::json!({}), &move || flag.load(Ordering::SeqCst), Instant::now() + Duration::from_secs(30)).unwrap_err();
    assert!(matches!(err, GhidraError::Cancelled), "{err:?}");
    assert!(t.elapsed() < Duration::from_secs(3));
    drop(client);
    fake.join();
}

#[test]
fn an_oversized_reply_is_refused_and_the_connection_closed() {
    let fake = Fake::start(Mode::Huge, "tok");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = Client::connect(fake.addr, "tok", deadline).expect("connect").with_max_reply(4 * 1024);
    match client.request("native_functions_list", serde_json::json!({}), &no_cancel(), deadline).unwrap_err() {
        GhidraError::Protocol(m) => assert!(m.contains("larger than"), "{m}"),
        other => panic!("expected a protocol error, got {other:?}"),
    }
    // The rest of that reply is still on the wire, so the client must not read on.
    match client.request("ping", serde_json::json!({}), &no_cancel(), Instant::now() + Duration::from_secs(2)).unwrap_err() {
        GhidraError::Connection(_) => {}
        other => panic!("expected a connection error on the closed socket, got {other:?}"),
    }
    drop(client);
    fake.join();
}

#[test]
fn a_reply_arriving_over_several_read_ticks_is_read_whole() {
    let fake = Fake::start(Mode::Split, "tok");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut client = Client::connect(fake.addr, "tok", deadline).expect("connect");
    let list = client.request("native_functions_list", serde_json::json!({}), &no_cancel(), deadline).expect("the whole reply");
    assert_eq!(list["functions"][0]["name"].as_str().map(str::len), Some(64 * 1024));
    drop(client);
    fake.join();
}

#[test]
fn dropping_the_client_closes_the_connection() {
    let fake = Fake::start(Mode::Silent, "tok");
    let saw_eof = fake.saw_eof.clone();
    {
        let client = Client::connect(fake.addr, "tok", Instant::now() + Duration::from_secs(5)).expect("connect");
        // Dropping here must close the socket so the server observes EOF.
        drop(client);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while !saw_eof.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(saw_eof.load(Ordering::SeqCst), "server did not see the connection close");
    fake.join();
}

#[test]
fn analysis_runs_once_with_the_language_pin_then_the_bridge_opens_it_read_only() {
    let pd = Path::new("/store/proj");
    let snap = Path::new("/store/target-abc");
    let log = Path::new("/store/ghidra.log");
    let strs = |v: Vec<OsString>| v.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<String>>();

    // No pin: Ghidra auto-detects, so neither flag appears. The analysis is saved (not read-only,
    // not deleted) and runs no script.
    let auto = strs(analyse_args(pd, snap, log, Path::new("/scripts"), None, None));
    assert!(auto.iter().any(|a| a == "-import"));
    assert!(!auto.iter().any(|a| a == "-processor"), "no processor flag without a pin");
    assert!(!auto.iter().any(|a| a == "-cspec"));
    assert!(!auto.iter().any(|a| a == "-readOnly" || a == "-deleteProject" || a == "-postScript"), "{auto:?}");
    // Our pre-script skips the slowest passes.
    let pre = auto.iter().position(|a| a == "-preScript").expect("runs the fast-analysis pre-script");
    assert_eq!(auto.get(pre + 1).map(String::as_str), Some(FAST_SCRIPT_NAME));

    // A pinned language id and compiler spec are forwarded, in order, with their values.
    let pinned = strs(analyse_args(pd, snap, log, Path::new("/scripts"), Some("RISCV:LE:64:RV64GC"), Some("gcc")));
    let pi = pinned.iter().position(|a| a == "-processor").expect("has -processor");
    assert_eq!(pinned.get(pi + 1).map(String::as_str), Some("RISCV:LE:64:RV64GC"));
    let ci = pinned.iter().position(|a| a == "-cspec").expect("has -cspec");
    assert_eq!(pinned.get(ci + 1).map(String::as_str), Some("gcc"));

    // A blank language id is ignored (treated as no pin); a cspec without a language is dropped.
    let blank = strs(analyse_args(pd, snap, log, Path::new("/scripts"), Some("  "), Some("gcc")));
    assert!(!blank.iter().any(|a| a == "-processor"));
    assert!(!blank.iter().any(|a| a == "-cspec"), "cspec needs a processor");

    // Later sessions open the saved program without analysing again, and run our bridge.
    let open = strs(bridge_args(pd, "target-abc", log, Path::new("/s.log"), Path::new("/scripts"), Path::new("/session.json")));
    let at = open.iter().position(|a| a == "-process").expect("opens the saved program");
    assert_eq!(open.get(at + 1).map(String::as_str), Some("target-abc"));
    assert!(open.iter().any(|a| a == "-noanalysis") && open.iter().any(|a| a == "-readOnly"));
    assert!(open.iter().any(|a| a == BRIDGE_SCRIPT_NAME) && !open.iter().any(|a| a == "-import"));
    assert!(!is_analysed(Path::new("/definitely/not/analysed")));
}
#[test]
fn ghidra_memory_follows_the_setting_within_its_bounds() {
    assert_eq!(auto_heap_gb(Some(64)), 31, "half of 64 GB, capped");
    assert_eq!(auto_heap_gb(Some(16)), 8);
    assert_eq!(auto_heap_gb(Some(4)), 4, "never below Ghidra's useful minimum");
    assert_eq!(auto_heap_gb(None), 8, "16 GB assumed when unknown");
    let cfg = |memory_gb| Config {
        analyze_headless: PathBuf::new(),
        java_home: None,
        startup_timeout: Duration::from_secs(1),
        store_dir: PathBuf::new(),
        language_id: None,
        compiler_spec: None,
        memory_gb,
    };
    assert_eq!(ghidra_heap(&cfg(Some(12))), "12G");
    assert_eq!(ghidra_heap(&cfg(Some(2))), "4G");
    assert_eq!(ghidra_heap(&cfg(Some(200))), "31G");
    assert_eq!(suggested_heap_gb(10 << 20), 4);
    assert_eq!(suggested_heap_gb(192 << 20), 8);
    assert_eq!(suggested_heap_gb(900 << 20), 16);
    assert_eq!(suggested_heap_gb(3 << 30), BIG_PROGRAM_GB);
}

#[test]
fn a_dot_free_store_is_given_to_ghidra_as_it_is() {
    let store = Path::new("/home/u/craft-anything/engines");
    assert_eq!(ghidra_root(store).unwrap(), store);
    assert!(has_dot_part(Path::new("/home/u/.config/craft-anything/engines")));
    assert!(!has_dot_part(store));
}

/// Removes the link [`ghidra_root`] makes for a test's store when the test ends, passed or not: it
/// lives outside the test's own temp folder, which is itself under a dot folder (`/tmp/.tmpXXXX`).
struct LinkGuard(PathBuf);

impl LinkGuard {
    /// The folder to give Ghidra for `store`, with a guard that removes its link.
    fn root(store: &Path) -> (LinkGuard, PathBuf) {
        (LinkGuard(store.to_path_buf()), ghidra_root(store).unwrap())
    }
}

impl Drop for LinkGuard {
    fn drop(&mut self) {
        if let Ok(link) = ghidra_root(&self.0)
            && link != self.0
            && std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink())
        {
            let _ = std::fs::remove_file(&link);
        }
    }
}

#[cfg(unix)]
#[test]
fn a_store_under_a_dot_folder_reaches_ghidra_through_a_dot_free_link() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join(".config").join("craft-anything").join("engines");
    let (_link, root) = LinkGuard::root(&store);
    assert!(!has_dot_part(&root), "{}", root.display());
    assert!(!has_dot_part(&project_cache_dir(&root, &"ab".repeat(32), &"cd".repeat(32))));
    // It is the store: what Ghidra saves there is in the store, and the next run finds the same link.
    std::fs::write(root.join("saved"), "x").unwrap();
    assert!(store.join("saved").is_file());
    assert_eq!(ghidra_root(&store).unwrap(), root);
    // A link left pointing elsewhere is replaced.
    std::fs::remove_file(&root).unwrap();
    std::os::unix::fs::symlink(home.path(), &root).unwrap();
    assert_eq!(ghidra_root(&store).unwrap(), root);
    assert_eq!(std::fs::read_link(&root).unwrap(), store);
}

#[test]
fn project_cache_dir_is_keyed_and_dot_free() {
    let dir = project_cache_dir(Path::new("/store"), &"ab".repeat(32), &"cd".repeat(32));
    let s = dir.to_string_lossy();
    assert!(s.contains("ghidra-cache"));
    assert!(!dir.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')));
}

#[test]
fn a_second_analysis_waits_for_the_running_one_and_a_dead_ones_lock_is_taken_over() {
    let d = tempfile::tempdir().unwrap();
    // A launcher that doesn't exist: if analyse() tried to run Ghidra itself, it would fail.
    let cfg = Config {
        analyze_headless: d.path().join("no-ghidra-here.bat"),
        java_home: None,
        startup_timeout: Duration::from_secs(30),
        store_dir: d.path().to_path_buf(),
        language_id: None,
        compiler_spec: None,
        memory_gb: None,
    };
    // The bytes and their real sha, so this reaches the launch instead of failing on a mismatch.
    let (sha, digest) = (craft_anything_evidence::sha256_hex(b"x"), "cd".repeat(32));
    // A temp folder can be under a dot folder (`/tmp/.tmpXXXX`): the project is then reached
    // through the store's link, which is the path analyse() gives back.
    let (_link, root) = LinkGuard::root(d.path());
    let project = project_cache_dir(&root, &sha, &digest);
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();

    // Someone is analysing (a fresh lock); their result appears a moment later.
    std::fs::write(lock_path(&project), "0").unwrap();
    let p2 = project.clone();
    let finisher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        std::fs::create_dir_all(&p2).unwrap();
        std::fs::write(p2.join(ANALYSED_MARKER), "target").unwrap();
    });
    let got = analyse(&cfg, b"x", &sha, &digest, &|| false, &|_, _| {});
    finisher.join().unwrap();
    assert_eq!(got.ok(), Some(project.clone()), "waited for the running analysis instead of starting another");

    // A lock left by a process that died goes stale and is taken over (here: an attempt to run).
    // What that process had half-written is moved aside, never deleted under it (#951).
    std::fs::remove_dir_all(&project).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("half-written.gpr"), "the other analysis may still be filling this").unwrap();
    let lock = std::fs::File::options().write(true).open(lock_path(&project)).unwrap();
    lock.set_modified(std::time::SystemTime::now() - LOCK_STALE - Duration::from_secs(5)).unwrap();
    drop(lock);
    // Seen while the analysis runs, not after: by then both paths have cleaned up.
    // The project sits beside its lock, which the store may reach through a link.
    let store = project.parent().unwrap().to_path_buf();
    let saw_aside = AtomicBool::new(false);
    let progress = |_: f32, msg: &str| {
        if msg.contains("Ghidra is analysing") {
            let aside = std::fs::read_dir(&store).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".orphaned-"));
            saw_aside.store(aside, Ordering::Relaxed);
        }
    };
    let err = analyse(&cfg, b"x", &sha, &digest, &|| false, &progress).unwrap_err();
    assert!(matches!(err, GhidraError::Launch(_)), "took over and tried to run Ghidra: {err:?}");
    assert!(!lock_path(&project).exists(), "its own lock is released when it fails");
    assert!(saw_aside.load(Ordering::Relaxed), "the folder the other analysis was filling must still be on disk, under its own name");
    assert!(!project.join("half-written.gpr").exists(), "it wrote into the folder the other analysis was filling");
}

/// Which refusals to move a folder are worth waiting on: Windows reports the live Ghidra this PR
/// is about as a permission problem, and everything else is the caller's to hear now.
#[test]
fn a_move_that_cannot_pass_is_told_apart_from_one_that_might() {
    use std::io::{Error, ErrorKind};
    assert!(move_may_pass(&Error::from(ErrorKind::PermissionDenied)), "a folder Windows has open files in reads as a permission problem");
    assert!(move_may_pass(&Error::from(ErrorKind::WouldBlock)), "a momentary block passes");
    // Windows reports a busy folder by number, not by kind: without these the wait would never
    // happen there, which is the whole point of the change.
    assert!(move_may_pass(&Error::from_raw_os_error(32)), "ERROR_SHARING_VIOLATION is a folder in use");
    assert!(move_may_pass(&Error::from_raw_os_error(33)), "ERROR_LOCK_VIOLATION is a file locked");
    assert!(!move_may_pass(&Error::from(ErrorKind::InvalidInput)), "a path we cannot name will not fix itself");
    assert!(!move_may_pass(&Error::from(ErrorKind::NotFound)), "gone is not a reason to wait");
    assert!(!move_may_pass(&Error::from_raw_os_error(2)), "a missing file is not a folder in use");
}

/// A lock that cannot be released reads to every other process as an analysis in progress, so
/// releasing it says why instead of failing silently.
#[test]
fn giving_back_a_lock_that_is_already_gone_is_not_a_failure() {
    let d = tempfile::tempdir().unwrap();
    assert!(release(&d.path().join("never-existed.analysing")).is_ok(), "nothing to release is not an error");
    let lock = d.path().join("held.analysing");
    std::fs::write(&lock, "").unwrap();
    assert!(release(&lock).is_ok());
    assert!(!lock.exists(), "the lock is gone once it is given back");
}

/// A move that cannot pass ends the attempt, so the lock goes back with it. Both outcomes are
/// checked here by making the release itself fail, which is a folder named as the lock file:
/// `remove_file` refuses that on every system, and no rename has to fail for the test to run.
#[test]
fn a_move_that_cannot_pass_gives_the_lock_back_and_says_why() {
    let d = tempfile::tempdir().unwrap();
    let project = d.path().join("target-abcd");
    let lock = d.path().join("held.analysing");
    std::fs::write(&lock, "").unwrap();

    let refusal = std::io::Error::from(std::io::ErrorKind::InvalidInput);
    let err = refuse_move(&lock, &project, &refusal);
    assert_eq!(
        err.to_string(),
        format!("could not start Ghidra: moving {} out of the way: {refusal}", project.display()),
        "a clean release leaves the move as the whole message"
    );
    assert!(!lock.exists(), "the lock is gone");

    // Now the lock cannot be released: a folder where the lock file should be.
    let stuck = d.path().join("stuck.analysing");
    std::fs::create_dir(&stuck).unwrap();
    let err = refuse_move(&stuck, &project, &refusal);
    let text = err.to_string();
    assert!(
        text.starts_with(&format!("could not start Ghidra: moving {} out of the way: {refusal}; and ", project.display())),
        "the move leads, then the lock: {text}"
    );
    assert!(text.contains("releasing the analysis lock"), "and the lock that could not be freed: {text}");
}

/// #951: the folder a stale lock was guarding is moved, not removed, so a Ghidra that outlived the
/// app that started it keeps writing into its own folder instead of into this analysis's.
#[test]
fn a_half_written_project_is_moved_aside_with_its_files() {
    let d = tempfile::tempdir().unwrap();
    let project = d.path().join("target-abcd");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("half-written.gpr"), "still being filled").unwrap();

    let aside = set_aside(&project).unwrap().expect("there was a folder to move");
    assert!(!project.exists(), "the folder this analysis needs is free");
    let moved =
        std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.path()).find(|p| p != &project).expect("the folder is still on disk, under a name of its own");
    assert!(moved.join("half-written.gpr").is_file(), "it kept the files the other analysis left");

    drop(aside);
    assert!(!moved.exists(), "and it goes once this analysis has its own folder");
    assert!(set_aside(&project).unwrap().is_none(), "nothing to move is not a failure");
}

#[test]
fn the_first_analysis_in_a_fresh_store_starts_instead_of_waiting_for_a_lock_it_cannot_take() {
    let d = tempfile::tempdir().unwrap();
    let cfg = Config {
        analyze_headless: d.path().join("no-ghidra-here.bat"),
        java_home: None,
        startup_timeout: Duration::from_secs(30),
        store_dir: d.path().join("fresh-store"),
        language_id: None,
        compiler_spec: None,
        memory_gb: None,
    };
    let _link = LinkGuard(cfg.store_dir.clone());
    let t = Instant::now();
    let err = analyse(&cfg, b"x", &"ab".repeat(32), &"cd".repeat(32), &|| false, &|_, _| {}).unwrap_err();
    assert!(matches!(err, GhidraError::Launch(_)), "tried to run Ghidra: {err:?}");
    assert!(t.elapsed() < Duration::from_secs(10), "took {:?}", t.elapsed());
}

/// A tiny x86-64 ELF whose two little functions are reached only through a table of pointers in
/// read-only data: `start` jumps through the table it is handed, so no code names them. Returns
/// the program and the addresses of `triple`, the second function and the table.
fn pointer_table_elf() -> (Vec<u8>, u64, u64, u64) {
    pointer_table_elf_at(0x40_0000, 2)
}

/// [`pointer_table_elf`] loaded at `base`, with ELF type `e_type` (2 executable, 3 position
/// independent).
fn pointer_table_elf_at(base: u64, e_type: u8) -> (Vec<u8>, u64, u64, u64) {
    let (start, triple, negate, table) = (base + 0x100, base + 0x110, base + 0x120, base + 0x1000);
    let mut data = triple.to_le_bytes().to_vec();
    data.extend(negate.to_le_bytes());
    let mut b = tiny_elf(base, e_type, start, &data);
    b[0x100..0x130].fill(0xcc);
    put(&mut b, 0x100, &[0x48, 0x8b, 0x47, 0x08, 0xff, 0xe0]); // start: mov rax,[rdi+8]; jmp rax
    put(&mut b, 0x110, &[0x8d, 0x04, 0x7f, 0xc3]); // triple: lea eax,[rdi+rdi*2]; ret
    put(&mut b, 0x120, &[0x89, 0xf8, 0xf7, 0xd8, 0xc3]); // negate: mov eax,edi; neg eax; ret
    (b, triple, negate, table)
}

fn put(b: &mut [u8], off: usize, bytes: &[u8]) {
    b[off..off + bytes.len()].copy_from_slice(bytes);
}

/// An x86-64 ELF loaded at `base` that starts at `start`: code goes in the first 0x1000 bytes,
/// `data` is read-only at `base + 0x1000`.
fn tiny_elf(base: u64, e_type: u8, start: u64, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 0x1000 + data.len()];
    // ELF header: ET_EXEC, x86-64, two program headers, no sections.
    put(&mut b, 0, b"\x7fELF\x02\x01\x01");
    put(&mut b, 16, &[e_type, 0, 62, 0, 1, 0, 0, 0]);
    put(&mut b, 24, &start.to_le_bytes());
    put(&mut b, 32, &0x40u64.to_le_bytes());
    put(&mut b, 52, &[64, 0, 56, 0, 2, 0, 64, 0]);
    // PT_LOAD r-x (headers and code), PT_LOAD r-- (the table).
    for (i, (flags, off, va, size)) in [(5u32, 0u64, base, 0x1000u64), (4, 0x1000, base + 0x1000, data.len() as u64)].into_iter().enumerate() {
        let o = 0x40 + i * 56;
        put(&mut b, o, &1u32.to_le_bytes());
        put(&mut b, o + 4, &flags.to_le_bytes());
        for (k, v) in [off, va, va, size, size, 0x1000].into_iter().enumerate() {
            put(&mut b, o + 8 + k * 8, &v.to_le_bytes());
        }
    }
    put(&mut b, 0x1000, data);
    b
}

/// Two functions that each jump on a byte through a table of offsets with no bounds check, the
/// tables back to back (as Rust's async state machines lay them out, #1715): read past its three
/// entries, `first`'s table runs into the second's. `first` also has a conditional jump that
/// bounds nothing before its dispatch, and loads another address between the table's `lea` and
/// the entry's read. `third` checks its byte first (`cmp`, `ja`), so its table is left to Ghidra.
/// Returns the program, `first` and `third`.
fn unguarded_jump_tables_elf() -> (Vec<u8>, u64, u64) {
    let base = 0x40_0000u64;
    let (first, second, third, table1, table2, table3) = (0x110usize, 0x140usize, 0x160usize, 0x1000usize, 0x100cusize, 0x1014usize);
    let lea = |code: &mut Vec<u8>, f: usize, reg: u8, to: usize| {
        code.extend([0x48, 0x8d, reg]);
        code.extend((to as i32 - (f + code.len() + 4) as i32).to_le_bytes());
    };
    let mut codes = Vec::new();
    let mut data = Vec::new();
    for (f, table, cases, noise) in [(first, table1, &[11u8, 22, 33][..], true), (second, table2, &[44, 55], false), (third, table3, &[66, 77], false)] {
        let mut code = Vec::new();
        if noise {
            code.extend([0x85, 0xf6, 0x75, 0x00]); // test esi,esi; jne (next): bounds nothing
        }
        code.extend([0x0f, 0xb6, 0x07]); // movzx eax,byte [rdi]
        if f == third {
            code.extend([0x83, 0xf8, 0x01, 0x77, 0x1c]); // cmp eax,1; ja (the ret after the cases)
        }
        lea(&mut code, f, 0x0d, table); // lea rcx,[rip+table]
        if noise {
            lea(&mut code, f, 0x15, table2); // lea rdx,[rip+table2]: another address on the way
        }
        code.extend([0x48, 0x63, 0x04, 0x81, 0x48, 0x01, 0xc8, 0xff, 0xe0]); // movsxd rax,[rcx+rax*4]; add rax,rcx; jmp rax
        for v in cases {
            data.extend(((f + code.len()) as i32 - table as i32).to_le_bytes());
            code.extend([0xb8, *v, 0, 0, 0, 0xc3]); // mov eax,v; ret
        }
        code.push(0xc3);
        codes.push((f, code));
    }
    data.extend([0u8; 8]);
    let mut b = tiny_elf(base, 2, base + 0x100, &data);
    b[0x100..0x1a0].fill(0xcc);
    put(&mut b, 0x100, &[0xe8, 0x0b, 0, 0, 0, 0xe8, 0x36, 0, 0, 0, 0xe8, 0x51, 0, 0, 0, 0xc3]); // start: call first, second, third; ret
    for (f, code) in codes {
        put(&mut b, f, &code);
    }
    (b, base + first as u64, base + third as u64)
}

/// Real Ghidra, only where `CRAFT_ANYTHING_TEST_GHIDRA` names the engine store holding `ghidra/`
/// and `jdk/` (the app's is `%APPDATA%/craft-anything/engines`). Its analysis goes to a temp folder.
#[test]
fn live_ghidra_makes_and_decompiles_code_reached_only_through_a_pointer_table() {
    let Some(store) = std::env::var_os("CRAFT_ANYTHING_TEST_GHIDRA").map(PathBuf::from) else { return };
    // Ghidra rejects project paths with a folder that starts with a dot, as tempdir()'s do.
    let temp = tempfile::Builder::new().prefix("ghidra-live").tempdir().unwrap();
    let cfg = Config {
        analyze_headless: get::headless(None, &store).expect("Ghidra in the engine store"),
        java_home: get::java_home(None, &store),
        startup_timeout: Duration::from_secs(600),
        store_dir: temp.path().to_path_buf(),
        language_id: None,
        compiler_spec: None,
        memory_gb: Some(4),
    };
    let (elf, triple, negate, table) = pointer_table_elf();
    let session = Session::start(&cfg, &elf, &craft_anything_evidence::sha256_hex(&elf), &"cd".repeat(32), &|| false, &|_, _| {}).expect("Ghidra starts");

    let known: Vec<u64> = session.list_functions().unwrap().iter().map(|f| f.entry).collect();
    assert!(!known.contains(&triple) && !known.contains(&negate), "the analysis already made these, so this tests nothing: {known:x?}");
    let made = session.decompile(triple, &|_| {}).unwrap().expect("an answer");
    assert!(made.created && made.pseudocode.as_deref().unwrap_or_default().contains("* 3"), "{made:?}");
    let again = session.decompile(negate, &|_| {}).unwrap().expect("an answer");
    assert!(again.created && again.pseudocode.as_deref().unwrap_or_default().contains("-param_1"), "{again:?}");
    match session.decompile(table, &|_| {}).unwrap_err() {
        GhidraError::Bridge { code, message } => assert!(code == "not_found" && message.contains("executable memory"), "{code}: {message}"),
        other => panic!("expected not_found, got {other:?}"),
    }
    // Made once, then found again, still marked as made on demand.
    assert!(session.decompile(triple, &|_| {}).unwrap().expect("an answer").created);
}

#[test]
fn addresses_move_between_our_image_base_and_the_engines() {
    // Ghidra loads a position-independent ELF at 0x100000; we read it at 0.
    let pie = Rebase::new(0, Some(0x10_0000));
    assert!(!pie.is_identity());
    assert_eq!((pie.to_engine(0x1160), pie.from_engine(0x10_1160)), (0x10_1160, 0x1160));
    // Below the base it's moved from (not in the program): left as it is, never wrapped.
    assert_eq!(pie.from_engine(0x40), 0x40);
    let near_top = Rebase::new(0, Some(u64::MAX - 1));
    assert_eq!(near_top.to_engine(0x10), 0x10, "no overflow");
    // Same base, or an engine that didn't say: nothing moves.
    for same in [Rebase::new(0x40_0000, Some(0x40_0000)), Rebase::new(0x40_0000, None)] {
        assert!(same.is_identity());
        assert_eq!((same.to_engine(0x40_1000), same.from_engine(0x40_1000)), (0x40_1000, 0x40_1000));
    }
}

/// Real Ghidra (as above): a position-independent ELF is loaded away from 0, the handshake says
/// where, and our addresses reach the right code once moved there.
#[test]
fn live_ghidra_reports_where_it_loaded_a_position_independent_elf() {
    let Some(store) = std::env::var_os("CRAFT_ANYTHING_TEST_GHIDRA").map(PathBuf::from) else { return };
    let temp = tempfile::Builder::new().prefix("ghidra-live").tempdir().unwrap();
    let cfg = Config {
        analyze_headless: get::headless(None, &store).expect("Ghidra in the engine store"),
        java_home: get::java_home(None, &store),
        startup_timeout: Duration::from_secs(600),
        store_dir: temp.path().to_path_buf(),
        language_id: None,
        compiler_spec: None,
        memory_gb: Some(4),
    };
    let (elf, triple, _, _) = pointer_table_elf_at(0, 3);
    let session = Session::start(&cfg, &elf, &craft_anything_evidence::sha256_hex(&elf), &"ef".repeat(32), &|| false, &|_, _| {}).expect("Ghidra starts");
    let base = session.image_base().expect("the handshake says where the program is");
    assert_ne!(base, 0, "Ghidra moves a program whose lowest segment is at 0");
    let rebase = Rebase::new(0, Some(base));
    let made = session.decompile(rebase.to_engine(triple), &|_| {}).unwrap().expect("an answer");
    assert_eq!(rebase.from_engine(made.entry), triple);
    assert!(made.pseudocode.as_deref().unwrap_or_default().contains("* 3"), "{made:?}");
}

/// Real Ghidra (as above): a table with no bounds check is cut to its real entries before the
/// decompile, so the switch has its three cases, not entries read from the next table (#1715).
#[test]
fn live_ghidra_bounds_a_jump_table_with_no_bounds_check() {
    let Some(store) = std::env::var_os("CRAFT_ANYTHING_TEST_GHIDRA").map(PathBuf::from) else { return };
    let temp = tempfile::Builder::new().prefix("ghidra-live").tempdir().unwrap();
    let cfg = Config {
        analyze_headless: get::headless(None, &store).expect("Ghidra in the engine store"),
        java_home: get::java_home(None, &store),
        startup_timeout: Duration::from_secs(600),
        store_dir: temp.path().to_path_buf(),
        language_id: None,
        compiler_spec: None,
        memory_gb: Some(4),
    };
    let (elf, first, third) = unguarded_jump_tables_elf();
    let session = Session::start(&cfg, &elf, &craft_anything_evidence::sha256_hex(&elf), &"ab".repeat(32), &|| false, &|_, _| {}).expect("Ghidra starts");
    let c = session.decompile(first, &|_| {}).unwrap().expect("an answer").pseudocode.unwrap_or_default();
    assert!(c.contains("manually overridden") && c.contains("0x21") && !c.contains("0x2c") && !c.contains("case 3"), "{c}");
    let guarded = session.decompile(third, &|_| {}).unwrap().expect("an answer").pseudocode.unwrap_or_default();
    assert!(!guarded.contains("manually overridden") && guarded.contains("0x4d"), "a guarded table is Ghidra's own: {guarded}");
}

#[test]
fn a_bridge_that_stops_early_leaves_its_log_beside_the_analysis() {
    let d = tempfile::tempdir().unwrap();
    let (log, script_log, dest) = (d.path().join("ghidra.log"), d.path().join("script.log"), d.path().join("target.bridge.log"));
    std::fs::write(&log, "ERROR could not open the project").unwrap();
    std::fs::write(&script_log, "bridge script failed").unwrap();
    assert_eq!(keep_log(&[&log, &script_log], &dest), dest);
    let kept = std::fs::read_to_string(&dest).unwrap();
    assert!(kept.contains("could not open the project") && kept.contains("bridge script failed"), "{kept}");
    // Nothing to copy: point at the first log rather than an empty file.
    let none = d.path().join("none.log");
    assert_eq!(keep_log(&[&none], &d.path().join("other.bridge.log")), none);
    assert!(!d.path().join("other.bridge.log").exists());
}

/// An install's decompiler folder, with this program in it.
fn install_with(root: &std::path::Path, folder: &[&str], exe: &str) {
    let mut dir = root.join("Ghidra").join("Features").join("Decompiler");
    for part in folder {
        dir = dir.join(part);
    }
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(exe), b"").unwrap();
}

#[test]
fn a_public_download_on_macos_has_no_decompiler_until_it_is_built() {
    let d = tempfile::tempdir().unwrap();
    install_with(d.path(), &["os", "linux_x86_64"], "decompile");
    install_with(d.path(), &["os", "win_x86_64"], "decompile.exe");
    assert!(!has_decompiler_for(d.path(), "mac_"), "other systems' programs don't count");
    assert!(has_decompiler_for(d.path(), "linux_"));
    assert!(has_decompiler_for(d.path(), "win_"));

    install_with(d.path(), &["build", "os", "mac_arm_64"], "decompile");
    assert!(has_decompiler_for(d.path(), "mac_"), "`gradlew buildNatives` puts it under build/os");
}

#[test]
fn an_empty_platform_folder_is_not_a_decompiler_and_an_unknown_layout_is_left_to_ghidra() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Ghidra/Features/Decompiler/os/mac_arm_64")).unwrap();
    assert!(!has_decompiler_for(d.path(), "mac_"));

    let unknown = tempfile::tempdir().unwrap();
    assert!(has_decompiler_for(unknown.path(), "mac_"), "never refused on a guess");
}
