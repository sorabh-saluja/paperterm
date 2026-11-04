//! Runs the real holder with a real shell, attaching and detaching as PaperTerm does.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

struct Holder {
    child: Child,
    socket: PathBuf,
}

impl Holder {
    fn start(name: &str) -> Holder {
        let socket = std::env::temp_dir().join(format!(
            "paperterm-test-{name}-{}.sock",
            std::process::id()
        ));
        let child = Command::new(env!("CARGO_BIN_EXE_paperterm"))
            .args(["--session-holder", "80", "24", "8", "16"])
            .env("PAPERTERM_SESSION_SOCKET", &socket)
            .env("PAPERTERM_SHELL", "/bin/sh")
            .env("PAPERTERM_SHELL_DIR", "/")
            .env("PS1", "$ ")
            .spawn()
            .unwrap();
        Holder { child, socket }
    }

    fn has_exited(&mut self) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
        let mut lock = self.socket.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

struct Client {
    stream: UnixStream,
    replay: String,
    seen: String,
}

impl Client {
    fn attach(socket: &Path) -> Client {
        let deadline = Instant::now() + WAIT;
        let mut stream = loop {
            match UnixStream::connect(socket) {
                Ok(stream) => break stream,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => panic!("no holder: {e}"),
            }
        };
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).unwrap();
        let mut replay = vec![0u8; u32::from_le_bytes(len) as usize];
        stream.read_exact(&mut replay).unwrap();
        Client {
            stream,
            replay: String::from_utf8_lossy(&replay).into_owned(),
            seen: String::new(),
        }
    }

    fn send(&mut self, kind: u8, payload: &[u8]) {
        self.stream.write_all(&[kind]).unwrap();
        self.stream
            .write_all(&(payload.len() as u32).to_le_bytes())
            .unwrap();
        self.stream.write_all(payload).unwrap();
    }

    fn run(&mut self, command: &str) {
        self.send(0, format!("{command}\n").as_bytes());
    }

    /// Waits for text that the command's echo can't contain, so write `$((…))` to it.
    fn wait_for(&mut self, text: &str) {
        let deadline = Instant::now() + WAIT;
        let mut buf = [0u8; 4096];
        while !self.seen.contains(text) {
            assert!(Instant::now() < deadline, "no {text:?} in {:?}", self.seen);
            match self.stream.read(&mut buf) {
                Ok(0) => panic!("session ended waiting for {text:?}: {:?}", self.seen),
                Ok(n) => self.seen.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(e)
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => panic!("read failed: {e}"),
            }
        }
    }

    fn at_end(&mut self) -> bool {
        let deadline = Instant::now() + WAIT;
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline {
            match self.stream.read(&mut buf) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(e)
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }
}

#[test]
fn the_shell_outlives_its_client_and_replays_what_it_missed() {
    let holder = Holder::start("survive");
    let mut first = Client::attach(&holder.socket);

    first.run("marker=kept; echo first-$((40+2))");
    first.wait_for("first-42");
    first.run("sleep 1; echo missed-$((40+3))");
    drop(first);
    std::thread::sleep(Duration::from_millis(1500));

    let mut second = Client::attach(&holder.socket);
    assert!(second.replay.contains("first-42"), "{:?}", second.replay);
    assert!(second.replay.contains("missed-43"), "{:?}", second.replay);

    second.run("echo still-$marker-$((40+4))");
    second.wait_for("still-kept-44");
}

#[test]
fn a_new_client_replaces_the_old_one() {
    let holder = Holder::start("replace");
    let mut old = Client::attach(&holder.socket);
    old.run("echo old-$((40+5))");
    old.wait_for("old-45");

    let mut new = Client::attach(&holder.socket);
    assert!(old.at_end());
    new.run("echo new-$((40+6))");
    new.wait_for("new-46");
}

#[test]
fn ending_the_session_stops_the_shell_and_the_holder() {
    let mut holder = Holder::start("end");
    let mut client = Client::attach(&holder.socket);
    client.run("echo up-$((40+7))");
    client.wait_for("up-47");

    client.send(2, &[]);
    assert!(client.at_end());
    assert!(holder.has_exited());
    assert!(!holder.socket.exists());
}

#[test]
fn exiting_the_shell_ends_the_session() {
    let mut holder = Holder::start("exit");
    let mut client = Client::attach(&holder.socket);
    client.run("exit");
    assert!(client.at_end());
    assert!(holder.has_exited());
}

#[test]
fn a_resize_reaches_the_shell() {
    let holder = Holder::start("resize");
    let mut client = Client::attach(&holder.socket);
    let size: Vec<u8> = [100u16, 30, 8, 16]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    client.send(1, &size);
    client.run("stty size; echo done-$((40+8))");
    client.wait_for("done-48");
    assert!(client.seen.contains("30 100"), "{:?}", client.seen);
}
