<div align="center">

<img src="assets/TY-DracoPho.svg" width="96" alt="DracoPho logo"/>

# dracopho-capture-core

**DracoPho Self-developed Screen Capture Core (Rust)**

A screen capture engine library that **does not rely on any "system built-in
screenshot" service**: only open standard protocols + self-written clients
(PipeWire screencast / wlr-screencopy) or self-written direct capture
(X11 XComposite/XGetImage).

`MIT OR Apache-2.0` · Rust library (for integration into apps like mark-shot)

Maintained by **Beijing Taiyin Zaowu Technology Co., Ltd. (北京太殷造物科技有限公司)**

</div>

## Features

- **Fully self-developed backends**: never calls xdg-desktop-portal Screenshot,
  GNOME Shell `screenshot_area`, KWin ScreenShot2 or any compositor built-in
  screenshot service.
- **Multiple capture modes**: full screen / region / all outputs /
  multiple windows / process / window component sub-region.
- **Authorize once, silent forever**: ScreenCast authorization is persisted via
  restore-token rotation, reused across processes and reboots — no repeated
  authorization dialogs.
- **Headless rule**: never creates windows, never pops dialogs, never disturbs
  other user processes.

## Feature Comparison

| Capability | **dracopho-capture-core** | System screenshot services¹ | External tool (grim) | Qt `grabWindow` |
| --- | --- | --- | --- | --- |
| Pure self-developed capture | ✅ | ❌ system service | ✅ (3rd-party) | ✅ (Qt framework) |
| Wayland (GNOME/KDE) | ✅ PipeWire screencast | ✅ portal | ❌ wlr-only | ❌ |
| Wayland wlroots (sway/hyprland/niri) | ✅ wlr-screencopy, no portal | ✅ portal | ✅ | ❌ |
| X11 full/region screen | ✅ self-written XGetImage | ✅ | ✅ | ✅ |
| X11 window own content (occluded/minimized) | ✅ XComposite named pixmap | ❌ | ❌ | ❌ |
| Window / component / multi-window selection | ✅ library API | ❌ | ❌ | ❌ |
| Process (pid/name) target | ✅ | ❌ | ❌ | ❌ |
| Headless silent (no window / no dialog) | ✅ strict | ❌ interactive portal | ✅ | ✅ |
| Authorization once, persistent across reboot | ✅ token rotation | ❌ | n/a | n/a |
| DMABUF GPU frames (EGL import) | ✅ self-written | — | — | — |
| No system screenshot dependency | ✅ **hard guarantee** | ❌ | ✅ | ✅ |

¹ xdg-desktop-portal Screenshot / GNOME Shell `screenshot_area` /
KWin ScreenShot2 — all **forbidden** by this project's rules.

## Hard Rules

- **Never call system built-in screenshots** (portal Screenshot /
  GNOME screenshot_area / KWin ScreenShot2).
- **Headless mode never pops dialogs**: on missing authorization it exits with
  an error and tells the caller how to authorize — it never triggers a
  compositor picker.
- Only open standard protocols: PipeWire (ScreenCast portal + self-written
  stream client), wlr-screencopy-unstable-v1 (self-written client), X11
  protocol (x11rb).

## Backend Matrix

| Backend | Coverage | Notes |
| --- | --- | --- |
| `pipewire-screencast` | GNOME / KDE / any Wayland compositor with ScreenCast portal | Self-written PipeWire client; DMABUF frames imported via self-written EGL (graceful fallback to shared memory without EGL) |
| `wlr-screencopy` | wlroots (sway / hyprland / niri / river…) | Self-written wlr-screencopy protocol client + wl_shm read; no portal, zero dialogs; region capture supported |
| `x11` | X11 session / XWayland | Self-written XGetImage full/region capture + XComposite named-pixmap window content (occluded/minimized windows included) |

Selection priority: `wlr-screencopy` → `pipewire-screencast` → `x11`, each
falling back to the next.

## Authorization Model (Key)

Wayland compositors (GNOME/KDE) only expose pixels through ScreenCast — **the
first use must be confirmed by the user once**. This library takes it to the
limit:

1. **Interactive authorization** (`allow_interactive_portal=true`, first time
   only) shows the picker once;
2. **Persistent permission**: `persist_mode=EXPLICITLY_REVOKED`, the portal
   stores the permission — survives reboots;
3. **Token auto-rotation**: `restore_token` is single-use; the library saves the
   new token returned by every `Start`, so the authorization chain continues
   indefinitely (across processes and reboots, until the portal permission is
   revoked);
4. **Resident process holds the session**: the library reuses the PipeWire
   session statically, so later captures in the same process are zero-dialog.

> Headless mode never starts a session on its own; it silently restores only
> while the saved token is valid, otherwise it errors out and asks for
> re-authorization — better to fail than to pop a dialog.

## Library API

The core deliverable is the **Rust library** (`dracopho_capture_core`); the CLI
is only a verification tool.

### Screen / region capture

```rust
use dracopho_capture_core::capture_types::{capture_frame, CaptureRequest};

// Full screen (set allow_interactive_portal=true on first integration to
// trigger the one-time authorization)
let req = CaptureRequest {
    source_geometry: None,
    allow_interactive_portal: true, // only first time; false afterwards
    ..Default::default()
};
let result = capture_frame(&req);
if let Some(img) = result.image {
    img.save("screen.png")?;
}

// Region (screen coordinates)
let req = CaptureRequest {
    source_geometry: Some((0, 0, 800, 600)),
    ..Default::default()
};

// All outputs (multi-monitor combined into one image)
let req = CaptureRequest { all_outputs: true, ..Default::default() };
```

### Window / process / component capture

```rust
use dracopho_capture_core::capture_types::{capture_windows, CaptureRequest};
use dracopho_capture_core::window::{parse_match, WindowMatch};

// Enumerate windows (self-written X11 / self-written GNOME extension)
for (i, w) in dracopho_capture_core::window::list_windows().iter().enumerate() {
    println!("[{i}] title={} class={} geo={:?}", w.title, w.class, w.geometry);
}

// Multiple windows (class + title substring + pid, repeatable)
let req = CaptureRequest {
    window_matches: vec![
        WindowMatch::Class("codium".to_string()),
        parse_match("DracoPho", Some("title"))?,
        WindowMatch::Pid(1234),
    ],
    ..Default::default()
};
for c in capture_windows(&req) {
    if let Some(img) = c.image { img.save(format!("{}.png", c.window.title))?; }
}

// Window component sub-region (window-relative coordinates)
let req = CaptureRequest {
    window_matches: vec![parse_match("codium", Some("class"))?],
    component: Some((0, 0, 200, 120)),
    ..Default::default()
};
```

`WindowMatch` supports `Id` / `Title` / `Class` / `Instance` / `Index` / `Pid` /
`Process` (/proc process name) / `Auto`. `parse_match(spec, by)` provides the
selector parsing consistent with C++ `--window-by`.

Full example: `examples/integration_demo.rs`.

## Integration Guide (host app, e.g. mark-shot)

1. **Resident process holds the session**: the library reuses the PipeWire
   session statically. The host app (mark-shot tray/daemon) calls with
   `allow_interactive_portal=true` once on startup; all later captures in the
   same process are **zero-dialog**.
2. **Persistent authorization**: tokens rotate automatically and are stored at
   `~/.config/dracopho-capture-core/screencast-token` (0600), surviving reboots;
   even if the host app restarts, the new process restores silently.
3. **Window content**: X11 uses XComposite (true window content); GNOME/Wayland
   falls back to full-screen frame + window-rect crop (occluded-window content
   may be unreliable; `WindowCapture` reports it honestly via `object_capture`
   / `error` fields).

## CLI (Verification Tool)

```bash
dracopho-capture --list-backends        # list available self-developed backends
dracopho-capture --list-windows         # list windows (JSON)
dracopho-capture --authorize            # interactive authorize once (saves token)
dracopho-capture --capture-to out.png   # headless capture (silent)
dracopho-capture --capture-to out.png --region 0,0,1920,1080 --include-cursor
dracopho-capture --capture-to dir --window VSCodium --window mark-shot \
                 --window-by auto --component 0,0,400,200
```

> The CLI is for verification and debugging; see the API and Integration Guide
> above for the proper library usage.

## Build

```bash
# Dep: libpipewire-0.3 dev (Debian/Ubuntu: apt install libpipewire-0.3-dev)
export PKG_CONFIG_PATH=/path/to/libpipewire/pkgconfig
cargo build --release
cargo test
```

> Build also needs clang/bindgen (libspa-sys binding generation); when missing
> set `LIBCLANG_PATH` and `BINDGEN_EXTRA_CLANG_ARGS` (pointing at clang builtin
> headers).

## Directory Layout

```
src/
  capture_types.rs        public types + backend dispatch (capture_frame / capture_windows)
  window.rs               window enumeration & selection (X11 / GNOME extension)
  egl_dmabuf.rs           DMABUF EGL import (dlopen, graceful downgrade without EGL)
  auth.rs                 authorization restore-token persistence
  backend/
    pipewire_screencast.rs  self-written PipeWire client (ScreenCast + EGL import)
    wlr_screencopy.rs       self-written wlr-screencopy client (wl_shm)
    x11.rs                  self-written X11 capture (XGetImage + XComposite)
  bin/dracopho_capture.rs CLI verification tool
examples/
  integration_demo.rs     library API integration example
assets/
  TY-DracoPho.svg         project logo (brand asset, copyright reserved)
```

## License

**`MIT OR Apache-2.0`** dual license (see `LICENSE`): integrators may choose
either, and may freely embed the library into their own applications (including
closed-source commercial ones), only keeping the copyright notice.

**Brand & logo reserved**: `assets/TY-DracoPho.svg` and the "DracoPho" /
"太殷龙摄" names and trademarks belong to the DracoPho project
(北京太殷造物科技有限公司). They are **not** granted by the license above and must
not be used for products or promotions unrelated to this project.

---

Copyright © 2026 Beijing Taiyin Zaowu Technology Co., Ltd.
