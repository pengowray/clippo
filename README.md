# clippo

Clipboard history for Wayland (tested on COSMIC) and X11 (tested on GNOME), with a history
window and OCR for copied images. Text and images you copy are stored in SQLite. Images show as
thumbnails in the window, and the text found in them is searchable.

## Desktops

| | Wayland (COSMIC) | X11 (GNOME on Xorg) | GNOME on Wayland |
|---|---|---|---|
| Recording history | yes | yes | yes, with the clippo extension |
| `clippo copy`, `plain`, `macro` | yes | yes | yes, with the clippo extension |
| Formatting (HTML, RTF) when pasting | yes | yes | no: plain text or the image only |
| Pasting | virtual keyboard or uinput | XTEST or uinput | the extension's virtual keyboard, or uinput |
| History window (`clippo window`) | yes | yes | yes |
| Emoji picker (`clippo emoji`) | yes | yes | yes |

Wayland needs `wl-clipboard` (`wl-paste`, `wl-copy`), and a compositor with a
data-control protocol (`ext_data_control_v1` or `zwlr_data_control`).

X11 needs the XFixes and XTEST extensions, which Xorg has.

GNOME on Wayland needs the clippo GNOME Shell extension, because GNOME's compositor (Mutter)
does not let other programs watch or set the clipboard. To install it:

```sh
ln -s "$PWD/gnome-extension/clippo@pengowray.github.io" ~/.local/share/gnome-shell/extensions/
```

Then log out and back in, and run:

```sh
gnome-extensions enable clippo@pengowray.github.io
```

GNOME loads a newly installed extension only at login. Until the extension is running,
`clippo watch` waits for it.

The extension offers only the main format of an entry (its text or image), so formatting
stored with an entry is lost when pasting on GNOME on Wayland.

On COSMIC, the history window and the emoji picker use the layer-shell protocol. On X11 and on
GNOME on Wayland they open as an ordinary window without a title bar, centred on the monitor
under the mouse pointer.

clippo picks the desktop from `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP`. `clippo watch`
prints the one it uses when it starts. To choose one yourself, set `CLIPPO_SESSION` to
`wayland`, `x11` or `gnome`.

## Build

```sh
cargo build --release   # OCR is very slow in debug builds
```

## Commands

| Command | What it does |
|---|---|
| `clippo watch` | Records clipboard history until stopped. On Wayland it runs `wl-paste --watch clippo ingest`; on X11 and GNOME on Wayland it runs `clippo ingest` after each copy. It also runs an OCR worker for new images, and listens on the service socket that the other clippo commands use. |
| `clippo ingest` | Stores clipboard data from stdin. `clippo watch` runs this. |
| `clippo window` | Opens the history window. Picking an entry copies it, then pastes it if `paste_on_select` is on. Running it again closes the window. `clippo menu` does the same. |
| `clippo emoji` | Opens the emoji and symbol picker in the same window: emoji by category, accented letters, Greek, Cyrillic, maths and other text symbols, searchable by name (`tau`, `e acute`, `:heart:`, `U+03C4`). Picking one pastes it without recording it in history. Alt+click adds a favourite. |
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
  `clippo watch`.
- On X11, the app you copied from keeps the clipboard data and sends it to apps that paste.
  When clippo puts an entry on the clipboard (`clippo copy`, the picker, `plain`, `macro`),
  a `clippo serve-clipboard` process sends the data until something else is copied, then exits.
  This process sets the primary selection too (the text a middle click pastes), because
  Shift+Insert pastes the primary selection in some X11 terminals, such as cosmic-term and xterm.

## Files

| Path | Contents |
|---|---|
| `$XDG_CONFIG_HOME/clippo/config.toml` | Config (optional) |
| `$XDG_DATA_HOME/clippo/history.db` | History database |
| `$XDG_DATA_HOME/clippo/ocrs/` | ocrs models |
| `$XDG_DATA_HOME/clippo/emoji.toml` | Emoji picker favourites, recent picks and skin tone |
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
method = "virtual"        # "virtual" (the Wayland virtual keyboard, or XTEST on X11), "uinput", or "auto" (tries "virtual", then "uinput")
auto_paste = true         # `clippo plain` pastes after replacing the clipboard
paste_on_select = true    # picking an entry in the history window pastes it
keys = "shift-insert"     # "shift-insert", "ctrl-v" or "ctrl-shift-v"
delay_ms = 100            # uinput only: wait before pasting, so the shortcut keys can be released
release_modifiers = true  # uinput method, and all methods on X11: release Super and Alt before pasting

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

Pasting presses keys through a virtual keyboard. On Wayland, the `virtual` method needs a
compositor with `zwp_virtual_keyboard_v1` (COSMIC, Sway, Hyprland). On X11, it uses the XTEST
extension. The method was called `wayland` before, and that name still works. The uinput method needs `/dev/uinput` to be writable by your user.
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

The same `clippo.service` file works on COSMIC and on GNOME. The desktop sets `XDG_SESSION_TYPE`
in the systemd user environment when you log in, and the service reads it from there.

## COSMIC shortcut

In COSMIC Settings, open Keyboard > Keyboard shortcuts > Custom shortcuts and add:

- Name: `Clipboard history`
- Command: `clippo window` (use the full path if it is not on `PATH`)
- Shortcut: `Super+V`

And for plain-text paste:

- Name: `Paste as plain text`
- Command: `clippo plain`
- Shortcut: `Super+Alt+V`

And for the emoji picker:

- Name: `Emoji`
- Command: `clippo emoji`
- Shortcut: `Super+;` (or `Super+.`, the Windows habit; both work inside the window too)

Macros can be bound the same way, for example `clippo macro 2` to paste today's date.

Remove or rebind any existing `Super+V` shortcut (such as one running cliphist) first.

## GNOME shortcuts

GNOME uses `Super+V` and `Super+M` to show the notification list. To free `Super+V`, leave the
notification list on `Super+M` only:

```sh
gsettings set org.gnome.shell.keybindings toggle-message-tray "['<Super>m']"
```

Then open Settings > Keyboard > View and Customize Shortcuts > Custom Shortcuts, and add:

- `clippo window` with the shortcut `Super+V`
- `clippo plain` with the shortcut `Super+Alt+V`
- `clippo emoji` with the shortcut `Super+;`
- `clippo macro 2` (or another macro number) on a shortcut of your choice

## License

MIT
