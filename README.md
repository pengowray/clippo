# clippo

Clipboard history for Wayland (tested on COSMIC), with a fuzzel picker and OCR for copied images.
Text and images you copy are stored in SQLite. Images show as thumbnails in the picker,
and the text found in them is searchable.

Requires `wl-clipboard` (`wl-paste`, `wl-copy`) and `fuzzel`. The compositor must support a
data-control protocol (`ext_data_control_v1` or `zwlr_data_control`).

## Build

```sh
cargo build --release   # OCR is very slow in debug builds
```

## Commands

| Command | What it does |
|---|---|
| `clippo watch` | Records clipboard history until stopped. Runs `wl-paste --watch clippo ingest` and an OCR worker for new images. |
| `clippo ingest` | Stores clipboard data from stdin. `wl-paste --watch` calls this. |
| `clippo menu` | Opens the history picker in fuzzel. Selecting an entry copies it, then pastes it if `paste_on_select` is on. If the picker is already open, closes it. |
| `clippo plain [--paste \| --no-paste]` | Replaces the clipboard with plain text, then pastes it if `auto_paste` is on. For an image, uses the text found in it. |
| `clippo get <id> [--plain]` | Writes an entry to stdout. With `--plain`, images output their OCR text (running OCR first if needed). |
| `clippo list` | Lists entries, newest first, as `id<TAB>label`. |
| `clippo delete <id>` | Deletes one entry. |
| `clippo clear` | Deletes all entries. |
| `clippo ocr <file>` | Prints the text found in an image file. Use `-` to read from stdin. |
| `clippo setup-ocr` | Downloads the ocrs models (about 12 MB) to `~/.local/share/clippo/ocrs/`. Skipped if they are already in `~/.cache/ocrs/`. |

Notes:

- Copying the same content again moves it to the top instead of adding a duplicate.
- Empty and whitespace-only copies are ignored, and so are copies that password managers
  mark as secret.
- When a copy offers both an image and text (for example, an image copied from a browser),
  the image is stored.
- Running `clippo ingest` by hand stores stdin as-is. It only checks the live clipboard
  (for image types and the password-manager hint) when run by `wl-paste --watch`.

## Files

| Path | Contents |
|---|---|
| `$XDG_CONFIG_HOME/clippo/config.toml` | Config (optional) |
| `$XDG_DATA_HOME/clippo/history.db` | History database |
| `$XDG_DATA_HOME/clippo/ocrs/` | ocrs models |
| `$XDG_CACHE_HOME/clippo/thumbs/` | Picker thumbnails |

## Config

Every key is optional. Defaults:

```toml
max_items = 1000          # oldest entries beyond this are deleted

[ocr]
engine = "auto"           # "ocrs", "tesseract", "auto" or "off"
tesseract_lang = "eng"    # passed to tesseract -l

[paste]
auto_paste = true         # `clippo plain` pastes after replacing the clipboard
paste_on_select = true    # `clippo menu` pastes the entry you pick
keys = "shift-insert"     # "shift-insert", "ctrl-v" or "ctrl-shift-v"
delay_ms = 100            # wait before pasting, so the shortcut keys can be released
release_modifiers = true  # send Super and Alt key-ups before pasting
```

Pasting presses keys through a virtual keyboard, so `/dev/uinput` must be writable by your user.
On Ubuntu and Pop!_OS, the `steam-devices` package sets this up. Shift+Insert pastes in most
apps, including terminals, which is why it's the default.

OCR engines:

- `ocrs`: built in. Needs the models from `clippo setup-ocr`.
- `tesseract`: runs the `tesseract` program. Install the `tesseract-ocr` package, plus
  `tesseract-ocr-<lang>` for languages other than English.
- `auto`: ocrs if its models are installed, otherwise tesseract if installed, otherwise no OCR.
  Images copied while no engine is available get OCR once one is.
- `off`: no OCR.

## Running it as a service

Only run one clipboard watcher at a time. If `cliphist` (or another history tool) is running,
stop it first: two watchers reading every copy can make apps hang on large images.

`~/.config/systemd/user/clippo.service`:

```ini
[Unit]
Description=clippo clipboard history
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=%h/.cargo/bin/clippo watch
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
```

Adjust `ExecStart` to wherever the binary is installed, then:

```sh
systemctl --user disable --now cliphist.service   # if used
systemctl --user daemon-reload
systemctl --user enable --now clippo.service
```

## COSMIC shortcut

In COSMIC Settings, open Keyboard > Keyboard shortcuts > Custom shortcuts and add:

- Name: `Clipboard history`
- Command: `clippo menu` (use the full path if it is not on `PATH`)
- Shortcut: `Super+V`

And for plain-text paste:

- Name: `Paste as plain text`
- Command: `clippo plain`
- Shortcut: `Super+Alt+V`

Remove or rebind any existing `Super+V` shortcut (such as one running cliphist) first.

## License

MIT
