//! Detects whether a hardware keyboard is attached, so the on-screen keyboard
//! can make way for it. Keys themselves arrive over qtfb: AppLoad forwards them
//! only to the focused window, whereas reading evdev would also pick up typing
//! meant for other apps.

/// Whether /proc/bus/input/devices lists a keyboard someone can type on.
pub fn present() -> bool {
    match std::fs::read_to_string("/proc/bus/input/devices") {
        Ok(data) => data.split("\n\n").any(is_typing_keyboard),
        Err(e) => {
            eprintln!("paperterm: hardware keyboard scan failed: {e}");
            false
        }
    }
}

fn is_typing_keyboard(block: &str) -> bool {
    let is_kbd = block
        .lines()
        .find_map(|line| line.strip_prefix("H: Handlers="))
        .is_some_and(|handlers| handlers.split_whitespace().any(|part| part == "kbd"));
    if !is_kbd {
        return false;
    }

    let name = block
        .lines()
        .find_map(|line| line.strip_prefix("N: Name=\""))
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.contains("powerkey")
        || name.contains("snvs")
        || name.contains("hall")
        || name.contains("elan marker")
        || name.contains("elan touch")
    {
        return false;
    }

    // The reMarkable Type Folio's bitmap is too short for the check below.
    if name == "rm_keyboard" {
        return true;
    }

    // Real keyboards advertise a broad EV_KEY bitmap. The built-in power key is
    // also tagged as `kbd`, but its KEY bitmap is tiny.
    block
        .lines()
        .filter_map(|line| line.strip_prefix("B: KEY="))
        .any(|keys| keys.split_whitespace().count() > 2 || keys.len() > 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_type_folio_and_rejects_the_power_key() {
        let folio = "N: Name=\"rM_Keyboard\"\nH: Handlers=kbd leds event4 \nB: KEY=40001e9000000000 77ffffffffffffe";
        assert!(is_typing_keyboard(folio));
        let power = "N: Name=\"30370000.snvs:snvs-powerkey\"\nH: Handlers=kbd event0 \nB: KEY=100000 0 0 0";
        assert!(!is_typing_keyboard(power));
    }

    #[test]
    fn accepts_a_bluetooth_keyboard_with_a_broad_bitmap() {
        let bt = "N: Name=\"temper Keyboard\"\nH: Handlers=sysrq kbd leds event4 \nB: KEY=1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe";
        assert!(is_typing_keyboard(bt));
    }
}
