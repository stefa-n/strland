# strpaper

A live wallpaper engine for the [Strland](https://github.com/strland) shell.
`strpaper` is a small, low-resource Windows application that renders a static,
animated or video wallpaper **behind the desktop icons** on every monitor.

It creates no visible window of its own: there is no taskbar button and no
Alt+Tab entry, and it does not require administrator privileges.

## Storage location

Wallpapers are stored in a dedicated, per-user directory:

```
%USERPROFILE%\.strland\strpaper\
```

For example:

```
C:\Users\SomeUser\.strland\strpaper\
```

`strpaper` resolves this path **dynamically at startup** from the current
Windows account's home directory (`%USERPROFILE%`). It is never taken from the
directory that contains `strpaper.exe`, so the same binary works for every
Windows account. The directory is created automatically if it does not exist.

```
%USERPROFILE%\.strland\
    strpaper\
        wallpaper.png
```

`strpaper` (the directory) is the dedicated wallpaper storage location for the
Strland shell.

## Supported files

The application looks only for files exactly named `wallpaper` with one of
these extensions:

| Extension | Type             |
| --------- | ---------------- |
| `.png`    | still image      |
| `.jpg`    | still image      |
| `.jpeg`   | still image      |
| `.bmp`    | still image      |
| `.gif`    | animated image   |
| `.mp4`    | video            |
| `.webm`   | video            |

It does **not** scan arbitrary directories — only the wallpaper directory
above, and only the exact file names listed.

## File selection priority

When more than one `wallpaper.*` file exists, the active one is chosen
deterministically:

1. Most recently modified `wallpaper.*` file wins (the file you last replaced
   is the one shown).
2. On an exact tie in modification time the extension order above is used
   (`.png` then `.jpg`, `.jpeg`, `.bmp`, `.gif`, `.mp4`, `.webm`).
3. If those still tie, the file path is compared lexicographically so the
   result is fully deterministic.

## Widgets

Scripts in `%USERPROFILE%\.strland\strpaper\widgets\*.rhai` run at ~30fps and
draw onto a shared transparent canvas composited over the wallpaper. They sit
above the wallpaper and below the desktop icons and all app windows.

```rhai
fn draw(pen) {
    pen.clear();
    pen.text(24, 24, "Hello!", 28, "#FFFFFF");
}
```

### Pen API

| Category | Methods |
|----------|---------|
| Drawing | `clear`, `fill_rect`, `fill_round_rect`, `fill_circle`, `circle`, `arc`, `line`, `text(x, y, str, px, color)`, `text(x, y, str, px, color, font)`, `text(x, y, str, px, color, font, spacing)` |
| Compositing | `set_opacity(a)` |
| System | `battery`, `charging`, `bt_count`, `bt_level(i)`, `bt_name(i)`, `cpu`, `ram`, `gpu` |
| Audio / Media | `audio_level(pos)`, `media_playing`, `media_title`, `media_artist` |
| Network | `http_get(url)`, `http_download(url, path)` |
| Process | `run(cmd)` |
| Images / Media | `image(x, y, w, h, path)`, `svg(x, y, w, h, path)`, `video(x, y, w, h, path)` |
| Time | `time(fmt)`, `date(fmt)` |
| Canvas | `width`, `height` |
| Regex | `regex_match(pattern, text)`, `regex_find(pattern, text)`, `regex_replace(pattern, text, repl)`, `regex_replace_all(pattern, text, repl)`, `regex_find_all(pattern, text)` |
| JSON | `json_parse(text)`, `json_stringify(val)`, `json_get(val, key)` |
| File I/O | `read_file(path)`, `write_file(path, content)`, `file_exists(path)` |

Colours: `"#RGB"`, `"#RRGGBB"`, `"#RRGGBBAA"` (alpha composites over the
wallpaper).

## Hot reload

The wallpaper directory is watched for changes:

- Replace `wallpaper.png` with a new `wallpaper.png` → the new image is
  reloaded automatically.
- Replace `wallpaper.png` with `wallpaper.mp4` → the engine switches to video
  playback automatically.
- Delete the wallpaper → the rendered wallpaper is removed and the application
  keeps running (waiting for a new wallpaper).
- If the directory does not exist at startup it is created and the engine
  waits until a wallpaper is placed there.

A short settle delay is used after a change so a partially-written file is not
read.

## Requirements

- Windows 10/11, x86_64 or aarch64.
- No administrator privileges.
- Decoder support for `.mp4`/`.webm` is provided by Windows Media Foundation
  (including H.264 / VP9 where the OS Media features are installed).

## Build

```
cargo build --release
```

The binary is `strpaper.exe` (built for the host target). Cross-build with
`cargo build --release --target <target>` for the target you need.

## Design notes

- **Rendering**: `strpaper` creates its own wallpaper window and makes it a
  *child* of the desktop's **background** WorkerW (the window that sits behind
  the desktop icons). Because that window is a child of the desktop, it is
  always painted underneath every application window — never above them. It is
  placed at the bottom of the desktop so the desktop icons still appear on top.
  When Explorer is absent, it attaches to the desktop window for custom-shell
  compatibility. The wallpaper is drawn into the window with GDI, once per
  monitor, using a "cover" (crop-to-fill) fit.
- **Painting**: drawing happens in `WM_PAINT`, so still images are only
  repainted when actually invalidated (reload / display change / being
  uncovered), never on a fixed timer. Animated and video wallpapers use a short
  timer to invalidate the window at ~30 fps.
- **Windowing**: the wallpaper window never activates and is transparent to
  mouse input. It is not a top-level window, so there is no taskbar button and
  no Alt+Tab entry.
- **Shutdown**: a console control handler makes `Ctrl+C` / `Ctrl+Break` and a
  console close shut the process down gracefully.
- **Video**: MP4/WebM are decoded with Windows Media Foundation **on a
  background thread**, so the UI thread is never blocked (no frozen wallpaper /
  busy cursor). Each decoded frame is published as a shared buffer and blitted
  by the UI thread. If the media cannot be decoded, the wallpaper window is
  hidden, the desktop is revealed, and the reason is written to the log.
- **Threading**: a small repaint timer plus a separate watcher detect
  filesystem changes.
- **Logging**: diagnostics are appended to `%USERPROFILE%\.strland\strpaper.log`
  (the app has no console). The log sits **outside** the watched `strpaper`
  directory so writing to it never re-triggers a wallpaper reload.
- **Resource usage**: static wallpapers only repaint when needed; only animated
  and video wallpapers spin a ~24-30 fps timer.
