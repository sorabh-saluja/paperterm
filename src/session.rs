//! Keeps the shell running after PaperTerm closes. A small background process
//! (the holder) owns the pty, and PaperTerm is only a screen attached to it over a
//! unix socket. The holder keeps recent output and replays it to each new client.
//!
//! Holder to client: u32 replay length, the replay, then raw pty output.
//! Client to holder: frames of kind u8, length u32, payload.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::os::fd::RawFd;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite, Options, Shell};
use polling::{Event as PollEvent, Events, PollMode, Poller};

/// The number changes with the protocol, so a holder left running by an older
/// build keeps working on its own socket instead of confusing a newer client.
const DEFAULT_SOCKET: &str = "/tmp/paperterm-session-1.sock";
const HOLDER_ARG: &str = "--session-holder";
const HISTORY_BYTES: usize = 1024 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(3);

const FRAME_INPUT: u8 = 0;
const FRAME_RESIZE: u8 = 1;
const FRAME_END: u8 = 2;
const FRAME_HEADER: usize = 5;

const KEY_PTY: usize = 0;
const KEY_SIGCHLD: usize = 1;
const KEY_LISTENER: usize = 2;
const KEY_CLIENT: usize = 3;

fn socket_path() -> PathBuf {
    std::env::var_os("PAPERTERM_SESSION_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

/// The arguments that make this binary run as the holder, if they are present.
pub fn holder_size(args: &[String]) -> Option<WindowSize> {
    if args.get(1).map(String::as_str) != Some(HOLDER_ARG) {
        return None;
    }
    let number = |i: usize, default: u16| {
        args.get(i)
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    };
    Some(WindowSize {
        num_cols: number(2, 80),
        num_lines: number(3, 24),
        cell_width: number(4, 8),
        cell_height: number(5, 16),
    })
}

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FRAME_HEADER + payload.len());
    bytes.push(kind);
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

/// Removes the next complete frame from `pending`.
fn take_frame(pending: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    let header = pending.get(..FRAME_HEADER)?;
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if pending.len() < FRAME_HEADER + len {
        return None;
    }
    let kind = pending[0];
    let payload = pending[FRAME_HEADER..FRAME_HEADER + len].to_vec();
    pending.drain(..FRAME_HEADER + len);
    Some((kind, payload))
}

fn encode_size(size: WindowSize) -> Vec<u8> {
    [size.num_cols, size.num_lines, size.cell_width, size.cell_height]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

fn decode_size(bytes: &[u8]) -> Option<WindowSize> {
    let value = |i: usize| Some(u16::from_le_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
    Some(WindowSize {
        num_cols: value(0)?,
        num_lines: value(2)?,
        cell_width: value(4)?,
        cell_height: value(6)?,
    })
}

/// Writes all of `bytes` to a non-blocking socket, giving up if the other end stalls.
fn write_all(stream: &UnixStream, bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut stream_ref = stream;
    let mut written = 0;
    while written < bytes.len() {
        match stream_ref.write(&bytes[written..]) {
            Ok(n) => written += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(ErrorKind::TimedOut.into());
                }
                let mut pollfd = libc::pollfd {
                    fd: stream.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                unsafe { libc::poll(&mut pollfd, 1, left.as_millis() as libc::c_int) };
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Recent output, trimmed to whole lines so a replay doesn't start mid-sequence.
#[derive(Default)]
struct History {
    bytes: Vec<u8>,
}

impl History {
    fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        if self.bytes.len() > 2 * HISTORY_BYTES {
            let cut = self.bytes.len() - HISTORY_BYTES;
            let start = self.bytes[cut..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(cut, |i| cut + i + 1);
            self.bytes.drain(..start);
        }
    }
}

struct FdSource(RawFd);
impl AsRawFd for FdSource {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

struct Attached {
    stream: UnixStream,
    pending: Vec<u8>,
    /// A new client may need a full-screen program to redraw, even at the same size.
    needs_redraw: bool,
}

fn shell_options() -> Options {
    let mut env = std::collections::HashMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("HOME".into(), "/home/root".into());
    env.insert(
        "PATH".into(),
        "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/home/root:/home/root/node/bin".into(),
    );
    let shell_path = std::env::var("PAPERTERM_SHELL").unwrap_or_else(|_| "/usr/bin/bash".into());
    let working_directory = std::env::var_os("PAPERTERM_SHELL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/root"));
    Options {
        shell: Some(Shell::new(shell_path, vec!["-l".into()])),
        working_directory: Some(working_directory),
        drain_on_exit: false,
        env,
    }
}

/// Stops a second holder from starting. The lock lives as long as the returned file.
fn holder_lock() -> io::Result<Option<File>> {
    let mut path = socket_path().into_os_string();
    path.push(".lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(locked.then_some(file))
}

/// Runs the shell until it exits or a client ends the session.
pub fn run_holder(size: WindowSize) -> io::Result<()> {
    let Some(_lock) = holder_lock()? else {
        return Ok(());
    };
    let path = socket_path();
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;

    tty::setup_env();
    let mut pty = tty::new(&shell_options(), size, 0)?;

    let poller = Arc::new(Poller::new()?);
    unsafe {
        pty.register(&poller, PollEvent::readable(KEY_PTY), PollMode::Level)?;
        poller.add_with_mode(
            &FdSource(listener.as_raw_fd()),
            PollEvent::readable(KEY_LISTENER),
            PollMode::Level,
        )?;
    }

    let mut events = Events::with_capacity(NonZeroUsize::new(16).unwrap());
    let mut buf = [0u8; 65536];
    let mut history = History::default();
    let mut client: Option<Attached> = None;
    // Typed input the shell isn't ready for yet.
    let mut input: Vec<u8> = Vec::new();

    'holder: loop {
        events.clear();
        let timeout = (!input.is_empty()).then(|| Duration::from_millis(20));
        match poller.wait(&mut events, timeout) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }

        for ev in events.iter() {
            match ev.key {
                KEY_PTY => loop {
                    match pty.reader().read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            history.push(&buf[..n]);
                            let sent = client
                                .as_ref()
                                .map(|attached| write_all(&attached.stream, &buf[..n]));
                            if matches!(sent, Some(Err(_))) {
                                detach(&poller, &mut client);
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) if e.kind() == ErrorKind::Interrupted => {}
                        Err(_) => break, // EIO: child gone; Exited event follows
                    }
                },
                KEY_SIGCHLD => {
                    if let Some(ChildEvent::Exited(_)) = pty.next_child_event() {
                        break 'holder;
                    }
                }
                KEY_LISTENER => {
                    while let Ok((stream, _)) = listener.accept() {
                        detach(&poller, &mut client);
                        if let Ok(attached) = attach(stream, &history, &poller) {
                            client = Some(attached);
                        }
                    }
                }
                KEY_CLIENT => {
                    let Some(attached) = client.as_mut() else {
                        continue;
                    };
                    let mut gone = false;
                    loop {
                        match (&attached.stream).read(&mut buf) {
                            Ok(0) => {
                                gone = true;
                                break;
                            }
                            Ok(n) => attached.pending.extend_from_slice(&buf[..n]),
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) if e.kind() == ErrorKind::Interrupted => {}
                            Err(_) => {
                                gone = true;
                                break;
                            }
                        }
                    }
                    while let Some((kind, payload)) = take_frame(&mut attached.pending) {
                        match kind {
                            FRAME_INPUT => input.extend_from_slice(&payload),
                            FRAME_RESIZE => {
                                if let Some(size) = decode_size(&payload) {
                                    pty.on_resize(size);
                                }
                                if std::mem::take(&mut attached.needs_redraw) {
                                    redraw_foreground(pty.reader().as_raw_fd());
                                }
                            }
                            FRAME_END => break 'holder,
                            _ => {}
                        }
                    }
                    if gone {
                        detach(&poller, &mut client);
                    }
                }
                _ => {}
            }
        }

        while !input.is_empty() {
            match pty.writer().write(&input) {
                Ok(n) => {
                    input.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break, // full, or the shell is gone: the Exited event follows
            }
        }
    }

    // Dropping the pty hangs up the shell if it is still running.
    let _ = fs::remove_file(&path);
    Ok(())
}

fn attach(stream: UnixStream, history: &History, poller: &Poller) -> io::Result<Attached> {
    stream.set_nonblocking(true)?;
    write_all(&stream, &(history.bytes.len() as u32).to_le_bytes())?;
    write_all(&stream, &history.bytes)?;
    unsafe {
        poller.add_with_mode(
            &FdSource(stream.as_raw_fd()),
            PollEvent::readable(KEY_CLIENT),
            PollMode::Level,
        )?;
    }
    Ok(Attached {
        stream,
        pending: Vec::new(),
        needs_redraw: true,
    })
}

fn detach(poller: &Poller, client: &mut Option<Attached>) {
    if let Some(attached) = client.take() {
        let _ = poller.delete(&attached.stream);
    }
}

fn redraw_foreground(pty_fd: RawFd) {
    unsafe {
        let group = libc::tcgetpgrp(pty_fd);
        if group > 0 {
            libc::kill(-group, libc::SIGWINCH);
        }
    }
}

/// PaperTerm's connection to the holder.
pub struct Session {
    stream: UnixStream,
}

impl Session {
    /// Connects to the running shell, starting one first if there is none. Also
    /// returns the output to replay, which must be parsed before anything else.
    pub fn connect_or_start(size: WindowSize) -> io::Result<(Session, Vec<u8>)> {
        let path = socket_path();
        let mut stream = UnixStream::connect(&path);
        if stream.is_err() {
            start_holder(size)?;
            let deadline = Instant::now() + START_TIMEOUT;
            loop {
                stream = UnixStream::connect(&path);
                if stream.is_ok() || Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let mut stream = stream?;

        stream.set_read_timeout(Some(WRITE_TIMEOUT))?;
        let mut len = [0u8; 4];
        stream.read_exact(&mut len)?;
        let mut replay = vec![0u8; u32::from_le_bytes(len) as usize];
        stream.read_exact(&mut replay)?;
        stream.set_read_timeout(None)?;
        stream.set_nonblocking(true)?;
        Ok((Session { stream }, replay))
    }

    pub fn raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.stream).read(buf)
    }

    pub fn send_input(&self, bytes: &[u8]) -> io::Result<()> {
        write_all(&self.stream, &frame(FRAME_INPUT, bytes))
    }

    pub fn resize(&self, size: WindowSize) -> io::Result<()> {
        write_all(&self.stream, &frame(FRAME_RESIZE, &encode_size(size)))
    }

    /// Ends the shell, rather than leaving it running for the next launch.
    pub fn end(&self) -> io::Result<()> {
        write_all(&self.stream, &frame(FRAME_END, &[]))
    }
}

/// Restarting xochitl kills everything in its cgroup, so the holder gets a systemd
/// unit of its own. Without systemd-run it still outlives PaperTerm closing.
fn start_holder(size: WindowSize) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let args = [
        HOLDER_ARG.to_string(),
        size.num_cols.to_string(),
        size.num_lines.to_string(),
        size.cell_width.to_string(),
        size.cell_height.to_string(),
    ];

    let in_own_unit = Command::new("systemd-run")
        .args(["--quiet", "--collect", "--unit=paperterm-session"])
        .arg(&exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if in_own_unit {
        return Ok(());
    }

    let mut holder = Command::new(&exe);
    holder
        .args(&args)
        .env_remove("LD_PRELOAD")
        .env_remove("QTFB_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        holder.pre_exec(|| {
            libc::setsid();
            // PaperTerm's window socket and instance lock must not stay open here.
            for fd in 3..256 {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            Ok(())
        });
    }
    holder.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_come_back_whole_and_in_order() {
        let mut pending = frame(FRAME_INPUT, b"ls\n");
        pending.extend(frame(FRAME_END, &[]));
        pending.extend(&frame(FRAME_INPUT, b"cut off")[..6]);

        assert_eq!(take_frame(&mut pending), Some((FRAME_INPUT, b"ls\n".to_vec())));
        assert_eq!(take_frame(&mut pending), Some((FRAME_END, Vec::new())));
        assert_eq!(take_frame(&mut pending), None);
        assert_eq!(pending.len(), 6);
    }

    #[test]
    fn a_size_survives_the_trip() {
        let size = decode_size(&encode_size(WindowSize {
            num_cols: 132,
            num_lines: 43,
            cell_width: 29,
            cell_height: 58,
        }))
        .unwrap();
        assert_eq!(
            (size.num_cols, size.num_lines, size.cell_width, size.cell_height),
            (132, 43, 29, 58)
        );
        assert!(decode_size(&[1, 2, 3]).is_none());
    }

    #[test]
    fn history_keeps_the_newest_output_from_a_line_start() {
        let mut history = History::default();
        let line = b"0123456789abcde\n";
        for _ in 0..(2 * HISTORY_BYTES / line.len() + 1) {
            history.push(line);
        }
        history.push(b"newest");

        assert!(history.bytes.len() <= HISTORY_BYTES + line.len() + 6);
        assert!(history.bytes.starts_with(line));
        assert!(history.bytes.ends_with(b"newest"));
    }

    #[test]
    fn only_the_holder_argument_starts_a_holder() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(holder_size(&args(&["paperterm"])).is_none());
        let size = holder_size(&args(&["paperterm", HOLDER_ARG, "100", "30"])).unwrap();
        assert_eq!((size.num_cols, size.num_lines), (100, 30));
    }
}
