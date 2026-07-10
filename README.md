# paperterm

A terminal emulator for the reMarkable Paper Pro with **pixel-wise partial e-ink updates**.

Built on `alacritty_terminal` (alacritty's headless core: VTE parsing, PTY, grid,
damage tracking) with a custom e-ink renderer speaking the qtfb shared-memory
framebuffer protocol, packaged as an AppLoad (xovi) app.

## Architecture

```
 shell (bash/ash)
   │ pty
   ▼
 alacritty_terminal::Term        ← full xterm/VTE state machine + damage tracking
   │ damaged cells only
   ▼
 renderer (embedded PSF2 bitmap font, no AA)
   │ pixel writes
   ▼
 qtfb shared-memory framebuffer  ← UPDATE_PARTIAL(x, y, w, h) per damage rect
   │ unix SEQPACKET /tmp/qtfb.sock
   ▼
 AppLoad/xochitl compositor → e-ink panel (only touched pixels refresh)
```

Input (touch / on-screen keyboard / physical keyboards) arrives over the same
qtfb channel; the OSK is drawn by us and partial-updated so typing never
flashes the screen.

## Install

PaperTerm requires a reMarkable Paper Pro with
[remagic](https://github.com/maximerivest/remagic) installed. Open the remagic
Store on the tablet and tap **PaperTerm**, or install it from a computer:

```sh
remagic install paperterm
```

The app is installed at `/home/root/xovi/exthome/appload/paperterm/`. Open it
from AppLoad. Run `exit` or use AppLoad's top-center close gesture to leave.

## Building

Install an aarch64 GNU cross-toolchain, then build and stage the AppLoad bundle:

```sh
cargo build --release --target aarch64-unknown-linux-gnu
./make-bundle.sh
```

The device uses glibc 2.39. The staged app is written to `dist/paperterm/` and
can be installed locally with `remagic install ./dist/paperterm`.

---

## The remagic family

Part of **[remagic](https://github.com/maximerivest/remagic)** — tools to make your
reMarkable Paper Pro magical:

- **[remagic](https://github.com/maximerivest/remagic)** — one-command installer:
  developer mode → AppLoad → your apps.
- **[riddle](https://github.com/maximerivest/riddle)** — an enchanted diary that
  writes back.
- **paperterm** — this terminal emulator.
- **[quill](https://github.com/maximerivest/quill)** — the takeover display host for
  instant-ink apps.
