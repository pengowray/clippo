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
| `clippo watch` | Records clipboard history until stopped. Runs `wl-paste --watch clippo ingest`, an OCR worker for new images, and the service socket the other commands use. |
| `clippo ingest` | Stores clipboard data from stdin. `wl-paste --watch` calls this. |
| `clippo menu` | Opens the history picker in fuzzel. Selecting an entry copies it, then pastes it if `paste_on_select` is on. If the picker is already open, closes it. |
| `clippo plain [--paste \| --no-paste] [--markdown \| --no-markdown]` | Replaces the clipboard with plain text, then pastes it if `auto_paste` is on. For an image, uses the text found in it. Text that looks like Markdown has the Markdown syntax removed when `plain.strip_markdown` is on; the flags override the setting. |
| `clippo copy <id> [--plain \| --no-markdown]` | Puts an entry on the clipboard without pasting. Offers every stored format (HTML, RTF) when the service is running. `--plain` offers only the text (an image's recognised text); `--no-markdown` offers the text with Markdown syntax removed. |
| `clippo get <id> [--plain] [--strip-markdown]` | Writes an entry to stdout. With `--plain`, images output their OCR text (running OCR first if needed). `--strip-markdown` removes Markdown syntax from text. |
| `clippo list [--recent]` | Lists entries, newest first, as `id<TAB>label`. `--recent` lists only entries used in the last 24 hours. |
| `clippo delete <id>` | Deletes one entry. Can be undone for a minute. |
| `clippo undelete <id>` | Brings back an entry deleted in the last minute. |
| `clippo clear` | Deletes all entries. |
| `clippo macro <n>` | Pastes macro `n` (1-based): the current time or date in a configured format. Not recorded in history; the previous clipboard is put back afterwards when `macros.restore_clipboard` is on. |
| `clippo macro --list` | Prints each macro as `n<TAB>value now<TAB>format[<TAB>label]`. |
| `clippo status` | Shows whether the service is running, which OCR engine it would use, and entry counts. |
| `clippo reload` | Makes the running service re-read the config file (OCR engine changes apply without a restart). |
| `clippo ocr <file>` | Prints the text found in an image file. Use `-` to read from stdin. |
| `clippo setup-ocr` | Downloads the ocrs models (about 12 MB) to `~/.local/share/clippo/ocrs/`. Skipped if they are already in `~/.cache/ocrs/`. |

Notes:

- Copying the same content again moves it to the top instead of adding a duplicate.
- Empty and whitespace-only copies are ignored, and so are copies that password managers
  mark as secret.
- When a copy offers both an image and text (for example, an image copied from a browser),
  the image is stored.
- Formatting is kept: `text/html` is stored beside the text when it has real formatting
  (bold, links, headings, lists, tables, images, code, or inline styles), `text/rtf` whenever
  it is offered, and for an image copied from a browser the `<img>` HTML. Each format is
  capped at 1 MB. Pasting from the picker or `clippo copy` offers all of them, so a rich
  editor gets its formatting back while a terminal gets the text. This needs the service:
  without it, only the primary type is offered.
- Entries not used for `expire_days` days are deleted, as are the oldest beyond `max_items`.
- Running `clippo ingest` by hand stores stdin as-is. It only checks the live clipboard
  (for image types, extra formats and the password-manager hint) when run by
  `wl-paste --watch`.

## Files

| Path | Contents |
|---|---|
| `$XDG_CONFIG_HOME/clippo/config.toml` | Config (optional) |
| `$XDG_DATA_HOME/clippo/history.db` | History database |
| `$XDG_DATA_HOME/clippo/ocrs/` | ocrs models |
| `$XDG_CACHE_HOME/clippo/thumbs/` | Picker thumbnails |
| `$XDG_STATE_HOME/clippo/clippo.log` | Errors from commands run by shortcuts |
| `$XDG_RUNTIME_DIR/clippo.sock` | Service socket |
| `$XDG_RUNTIME_DIR/clippo/skip` | Hashes of copies `ingest` must not record (macro pastes) |

## Config

Every key is optional. Defaults:

```toml
max_items = 1000          # oldest entries beyond this are deleted
expire_days = 7           # delete entries not used for this many days; 0 keeps everything

[ocr]
engine = "auto"           # "ocrs", "tesseract", "auto" or "off"
tesseract_lang = "eng"    # passed to tesseract -l

[paste]
method = "wayland"        # "wayland" (virtual keyboard protocol), "uinput", or "auto" = wayland then uinput
auto_paste = true         # `clippo plain` pastes after replacing the clipboard
paste_on_select = true    # `clippo menu` pastes the entry you pick
keys = "shift-insert"     # "shift-insert", "ctrl-v" or "ctrl-shift-v"
delay_ms = 100            # uinput only: wait before pasting, so the shortcut keys can be released
release_modifiers = true  # uinput only: send Super and Alt key-ups before pasting

[plain]
strip_markdown = true     # `clippo plain` also removes Markdown syntax when the text looks like Markdown

[macros]
restore_clipboard = true  # put the previous clipboard back after `clippo macro`

# Setting any [[macros.items]] replaces these three defaults. `label` is optional.
[[macros.items]]
format = "%H:%M"          # strftime; see https://docs.rs/chrono/latest/chrono/format/strftime/
[[macros.items]]
format = "%Y-%m-%d"
[[macros.items]]
format = "%Y-%m-%d %H:%M:%S"
```

Pasting presses keys through a virtual keyboard. The Wayland method needs a compositor with
`zwp_virtual_keyboard_v1` (COSMIC, Sway, Hyprland). The uinput method needs `/dev/uinput` to be writable by your user.
On Ubuntu and Pop!_OS, the `steam-devices` package sets this up. Shift+Insert pastes in most
apps, including terminals, which is why it's the default.

Markdown is only removed from text that looks like Markdown: two or more of a heading, a
fenced code block, a link or image, paired emphasis, two list lines, a block quote, or a
table separator. Code blocks are kept as they are. Bullets become `- `; numbered items stay.

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

The service also serves the clipboard for multi-format pastes, so restarting it while such a
paste is on the clipboard clears the clipboard.

## COSMIC shortcut

In COSMIC Settings, open Keyboard > Keyboard shortcuts > Custom shortcuts and add:

- Name: `Clipboard history`
- Command: `clippo menu` (use the full path if it is not on `PATH`)
- Shortcut: `Super+V`

And for plain-text paste:

- Name: `Paste as plain text`
- Command: `clippo plain`
- Shortcut: `Super+Alt+V`

Macros can be bound the same way, for example `clippo macro 2` to paste today's date.

Remove or rebind any existing `Super+V` shortcut (such as one running cliphist) first.

## License

MIT
