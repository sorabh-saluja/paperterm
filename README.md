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

## Device layout

- Binary + manifest install to `/home/root/xovi/exthome/appload/paperterm/`
- Requires xovi + AppLoad (the qtfb server lives in appload.so)

## Building

Cross-compiled from x86_64 to `aarch64-unknown-linux-gnu` (device: glibc 2.39).
See `docs/` and `.cargo/config.toml` once toolchain research lands.

---

## The remagic family

Part of **[remagic](https://github.com/YOUR-USER/remagic)** — tools to make your
reMarkable Paper Pro magical:

- **[remagic](https://github.com/YOUR-USER/remagic)** — one-command installer:
  developer mode → AppLoad → your apps.
- **[riddle](https://github.com/YOUR-USER/riddle)** — an enchanted diary that
  writes back.
- **paperterm** — this terminal emulator.
- **[quill](https://github.com/YOUR-USER/quill)** — the takeover display host for
  instant-ink apps.
