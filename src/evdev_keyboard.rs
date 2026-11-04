//! Minimal direct evdev keyboard reader for Bluetooth/USB keyboards.
//!
//! This is a deployable PaperTerm-side test path for hardware keyboards. It
//! reads `/dev/input/event*` devices advertised as `kbd` in
//! `/proc/bus/input/devices` and emits the same KeyAction values as AppLoad's
//! virtual keyboard path.

use std::collections::BTreeSet;
use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::keys::KeyAction;

pub const POLL_KEY_BASE: usize = 20;

const EV_KEY: u16 = 0x01;
const INPUT_EVENT_SIZE: usize = 24; // aarch64: timeval(16) + type(2) + code(2) + value(4)

#[derive(Debug)]
struct Device {
    fd: RawFd,
    key: usize,
    path: PathBuf,
    name: String,
}

#[derive(Debug)]
pub struct EvdevKeyboard {
    devices: Vec<Device>,
    next_key: usize,
    shift: bool,
    ctrl: bool,
    alt: bool,
}

impl Default for EvdevKeyboard {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            next_key: POLL_KEY_BASE,
            shift: false,
            ctrl: false,
            alt: false,
        }
    }
}

impl EvdevKeyboard {
    pub fn open() -> io::Result<Self> {
        let mut this = Self::default();
        this.open_new_devices()?;
        Ok(this)
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn fds(&self) -> impl Iterator<Item = (usize, RawFd)> + '_ {
        self.devices.iter().map(|d| (d.key, d.fd))
    }

    pub fn rescan(&mut self) -> io::Result<Vec<(usize, RawFd)>> {
        let paths = keyboard_event_paths()?;
        let current: BTreeSet<PathBuf> = paths.iter().map(|(path, _)| path.clone()).collect();
        let mut i = 0;
        while i < self.devices.len() {
            if current.contains(&self.devices[i].path) {
                i += 1;
            } else {
                let dev = self.devices.remove(i);
                eprintln!(
                    "paperterm: keyboard input removed {} ({})",
                    dev.path.display(),
                    dev.name
                );
                unsafe { libc::close(dev.fd) };
            }
        }

        self.open_new_devices_from(paths)
    }

    fn open_new_devices(&mut self) -> io::Result<Vec<(usize, RawFd)>> {
        let paths = keyboard_event_paths()?;
        self.open_new_devices_from(paths)
    }

    fn open_new_devices_from(
        &mut self,
        paths: Vec<(PathBuf, String)>,
    ) -> io::Result<Vec<(usize, RawFd)>> {
        let mut added = Vec::new();
        for (path, name) in paths {
            if self.devices.iter().any(|d| d.path == path) {
                continue;
            }
            let c_path = CString::new(path.as_os_str().as_bytes())?;
            let fd = unsafe {
                libc::open(
                    c_path.as_ptr(),
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                eprintln!(
                    "paperterm: could not open keyboard input {}: {}",
                    path.display(),
                    io::Error::last_os_error()
                );
                continue;
            }
            let key = self.next_key;
            self.next_key += 1;
            eprintln!(
                "paperterm: using keyboard input {} ({})",
                path.display(),
                name
            );
            self.devices.push(Device {
                fd,
                key,
                path,
                name,
            });
            added.push((key, fd));
        }
        Ok(added)
    }

    pub fn drain_poll_key(&mut self, poll_key: usize) -> io::Result<Vec<(KeyAction, bool, bool)>> {
        let Some(device) = self.devices.iter().find(|d| d.key == poll_key) else {
            return Ok(Vec::new());
        };

        let fd = device.fd;
        let mut out = Vec::new();
        loop {
            let mut buf = [0u8; INPUT_EVENT_SIZE * 32];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    return Ok(out);
                }
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if n == 0 {
                return Ok(out);
            }

            for chunk in buf[..n as usize].chunks_exact(INPUT_EVENT_SIZE) {
                let ev_type = u16::from_ne_bytes(chunk[16..18].try_into().unwrap());
                let code = u16::from_ne_bytes(chunk[18..20].try_into().unwrap());
                let value = i32::from_ne_bytes(chunk[20..24].try_into().unwrap());
                if ev_type != EV_KEY {
                    continue;
                }
                if self.update_modifier(code, value) {
                    continue;
                }
                if value == 1 || value == 2 {
                    if let Some(action) = map_key(code, self.shift) {
                        out.push((action, self.ctrl, self.alt));
                    }
                }
            }
        }
    }

    fn update_modifier(&mut self, code: u16, value: i32) -> bool {
        let pressed = value != 0;
        match code {
            42 | 54 => self.shift = pressed, // KEY_LEFTSHIFT / KEY_RIGHTSHIFT
            29 | 97 => self.ctrl = pressed,  // KEY_LEFTCTRL / KEY_RIGHTCTRL
            56 | 100 => self.alt = pressed,  // KEY_LEFTALT / KEY_RIGHTALT
            _ => return false,
        }
        true
    }
}

impl Drop for EvdevKeyboard {
    fn drop(&mut self) {
        for dev in &self.devices {
            unsafe { libc::close(dev.fd) };
        }
    }
}

fn keyboard_event_paths() -> io::Result<Vec<(PathBuf, String)>> {
    let data = std::fs::read_to_string("/proc/bus/input/devices")?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    for block in data.split("\n\n") {
        let handlers = block
            .lines()
            .find_map(|line| line.strip_prefix("H: Handlers="));
        let Some(handlers) = handlers else { continue };
        if !handlers.split_whitespace().any(|part| part == "kbd") {
            continue;
        }
        let name = block
            .lines()
            .find_map(|line| line.strip_prefix("N: Name=\""))
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or("unknown keyboard")
            .to_string();
        if !looks_like_typing_keyboard(block, &name) {
            continue;
        }
        for part in handlers.split_whitespace() {
            if part.starts_with("event") && seen.insert(part.to_string()) {
                out.push((PathBuf::from(format!("/dev/input/{part}")), name.clone()));
            }
        }
    }

    Ok(out)
}

fn looks_like_typing_keyboard(block: &str, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("powerkey")
        || lower.contains("snvs")
        || lower.contains("hall")
        || lower.contains("elan marker")
        || lower.contains("elan touch")
    {
        return false;
    }

    // The reMarkable Type Folio's bitmap is too short for the check below.
    if lower == "rm_keyboard" {
        return true;
    }

    // Real keyboards advertise a broad EV_KEY bitmap. The built-in power key is
    // also tagged as `kbd`, but its KEY bitmap is tiny.
    block
        .lines()
        .filter_map(|line| line.strip_prefix("B: KEY="))
        .any(|keys| keys.split_whitespace().count() > 2 || keys.len() > 32)
}

fn map_key(code: u16, shift: bool) -> Option<KeyAction> {
    Some(match code {
        1 => KeyAction::Esc,
        14 => KeyAction::Backspace,
        15 => KeyAction::Tab,
        28 => KeyAction::Enter,
        102 => KeyAction::Home,
        103 => KeyAction::Up,
        104 => KeyAction::PgUp,
        105 => KeyAction::Left,
        106 => KeyAction::Right,
        107 => KeyAction::End,
        108 => KeyAction::Down,
        109 => KeyAction::PgDn,
        111 => KeyAction::Delete,
        _ => KeyAction::Char(map_char(code, shift)?),
    })
}

fn map_char(code: u16, shift: bool) -> Option<char> {
    let ch = match code {
        2 => {
            if shift {
                '!'
            } else {
                '1'
            }
        }
        3 => {
            if shift {
                '@'
            } else {
                '2'
            }
        }
        4 => {
            if shift {
                '#'
            } else {
                '3'
            }
        }
        5 => {
            if shift {
                '$'
            } else {
                '4'
            }
        }
        6 => {
            if shift {
                '%'
            } else {
                '5'
            }
        }
        7 => {
            if shift {
                '^'
            } else {
                '6'
            }
        }
        8 => {
            if shift {
                '&'
            } else {
                '7'
            }
        }
        9 => {
            if shift {
                '*'
            } else {
                '8'
            }
        }
        10 => {
            if shift {
                '('
            } else {
                '9'
            }
        }
        11 => {
            if shift {
                ')'
            } else {
                '0'
            }
        }
        12 => {
            if shift {
                '_'
            } else {
                '-'
            }
        }
        13 => {
            if shift {
                '+'
            } else {
                '='
            }
        }
        16 => letter('q', shift),
        17 => letter('w', shift),
        18 => letter('e', shift),
        19 => letter('r', shift),
        20 => letter('t', shift),
        21 => letter('y', shift),
        22 => letter('u', shift),
        23 => letter('i', shift),
        24 => letter('o', shift),
        25 => letter('p', shift),
        26 => {
            if shift {
                '{'
            } else {
                '['
            }
        }
        27 => {
            if shift {
                '}'
            } else {
                ']'
            }
        }
        30 => letter('a', shift),
        31 => letter('s', shift),
        32 => letter('d', shift),
        33 => letter('f', shift),
        34 => letter('g', shift),
        35 => letter('h', shift),
        36 => letter('j', shift),
        37 => letter('k', shift),
        38 => letter('l', shift),
        39 => {
            if shift {
                ':'
            } else {
                ';'
            }
        }
        40 => {
            if shift {
                '"'
            } else {
                '\''
            }
        }
        41 => {
            if shift {
                '~'
            } else {
                '`'
            }
        }
        43 => {
            if shift {
                '|'
            } else {
                '\\'
            }
        }
        44 => letter('z', shift),
        45 => letter('x', shift),
        46 => letter('c', shift),
        47 => letter('v', shift),
        48 => letter('b', shift),
        49 => letter('n', shift),
        50 => letter('m', shift),
        51 => {
            if shift {
                '<'
            } else {
                ','
            }
        }
        52 => {
            if shift {
                '>'
            } else {
                '.'
            }
        }
        53 => {
            if shift {
                '?'
            } else {
                '/'
            }
        }
        57 => ' ',
        _ => return None,
    };
    Some(ch)
}

fn letter(c: char, shift: bool) -> char {
    if shift {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_type_folio_and_rejects_the_power_key() {
        let folio = "N: Name=\"rM_Keyboard\"\nH: Handlers=kbd leds event4 \nB: KEY=40001e9000000000 77ffffffffffffe";
        assert!(looks_like_typing_keyboard(folio, "rM_Keyboard"));
        let power = "N: Name=\"30370000.snvs:snvs-powerkey\"\nH: Handlers=kbd event0 \nB: KEY=100000 0 0 0";
        assert!(!looks_like_typing_keyboard(power, "30370000.snvs:snvs-powerkey"));
    }

    #[test]
    fn maps_ctrl_c_keys() {
        assert_eq!(map_key(46, false), Some(KeyAction::Char('c')));
        assert_eq!(map_key(28, false), Some(KeyAction::Enter));
    }
}
