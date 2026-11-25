// clippo helper for GNOME on Wayland, where other programs cannot watch or set the clipboard.
// A D-Bus service (io.github.pengowray.clippo.Shell at /io/github/pengowray/clippo/Shell) that:
// - signals each clipboard change, and gives the offered types and data on request
// - takes the clipboard (with one format)
// - presses the paste keys through a virtual keyboard
// It also centres clippo's history window on the monitor under the pointer, keeps it on top
// and focuses it.
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const NAME = 'io.github.pengowray.clippo.Shell';
const PATH = '/io/github/pengowray/clippo/Shell';
const IFACE = `<node>
  <interface name="${NAME}1">
    <!-- Another app (or clippo) took the clipboard. -->
    <signal name="OwnerChanged"/>
    <!-- Types the clipboard offers, e.g. text/plain;charset=utf-8, image/png. -->
    <method name="ListTypes"><arg type="as" direction="out"/></method>
    <!-- The clipboard's data as one type; empty if that type is not offered. -->
    <method name="Read"><arg type="s" direction="in"/><arg type="ay" direction="out"/></method>
    <!-- Take the clipboard with these (type, data) pairs; the first is the main one. Only the
         first is offered for now: GJS cannot implement a selection source with several types. -->
    <method name="SetContent"><arg type="a(say)" direction="in"/></method>
    <!-- Press shift-insert, ctrl-v or ctrl-shift-v, once Super and Alt are released. -->
    <method name="PressKeys"><arg type="s" direction="in"/></method>
    <property name="Version" type="u" access="read"/>
  </interface>
</node>`;
const VERSION = 1;

// The window clippo opens for its history and emoji picker (its Wayland app id).
const CLIPPO_APP_ID = 'clippo';

const KEYS = {
    'shift-insert': [Clutter.KEY_Shift_L, Clutter.KEY_Insert],
    'ctrl-v': [Clutter.KEY_Control_L, Clutter.KEY_v],
    'ctrl-shift-v': [Clutter.KEY_Control_L, Clutter.KEY_Shift_L, Clutter.KEY_v],
};
// Modifiers a shortcut may still be holding when clippo asks for a paste.
const HELD = Clutter.ModifierType.MOD4_MASK | Clutter.ModifierType.SUPER_MASK |
    Clutter.ModifierType.MOD1_MASK | Clutter.ModifierType.META_MASK |
    Clutter.ModifierType.HYPER_MASK;
const RELEASE_WAIT_MS = 1000;

export default class ClippoExtension extends Extension {
    enable() {
        this._selection = global.display.get_selection();
        this._ownerChanged = this._selection.connect('owner-changed', (_sel, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD)
                this._dbus.emit_signal('OwnerChanged', null);
        });
        this._windowCreated = global.display.connect('window-created',
            (_display, win) => this._placeIfClippo(win));
        const seat = Clutter.get_default_backend().get_default_seat();
        this._keyboard = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);

        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._dbus.export(Gio.DBus.session, PATH);
        this._nameId = Gio.bus_own_name(Gio.BusType.SESSION, NAME,
            Gio.BusNameOwnerFlags.REPLACE, null, null, null);
    }

    disable() {
        Gio.bus_unown_name(this._nameId);
        this._dbus.unexport();
        this._dbus = null;
        this._selection.disconnect(this._ownerChanged);
        global.display.disconnect(this._windowCreated);
        this._keyboard = null;
        if (this._releaseWait) {
            GLib.source_remove(this._releaseWait);
            this._releaseWait = 0;
        }
    }

    get Version() {
        return VERSION;
    }

    ListTypes() {
        return this._selection.get_mimetypes(Meta.SelectionType.SELECTION_CLIPBOARD);
    }

    ReadAsync([mimetype], invocation) {
        const reply = bytes => invocation.return_value(new GLib.Variant('(ay)', [bytes]));
        if (!this.ListTypes().includes(mimetype)) {
            reply(new Uint8Array());
            return;
        }
        const out = Gio.MemoryOutputStream.new_resizable();
        this._selection.transfer_async(Meta.SelectionType.SELECTION_CLIPBOARD, mimetype, -1,
            out, null, (sel, res) => {
                try {
                    sel.transfer_finish(res);
                    out.close(null);
                    reply(out.steal_as_bytes().toArray());
                } catch (e) {
                    invocation.return_dbus_error(`${NAME}.Error.ReadFailed`, e.message);
                }
            });
    }

    SetContent(formats) {
        if (formats.length === 0)
            throw new GLib.Error(Gio.IOErrorEnum, Gio.IOErrorEnum.INVALID_ARGUMENT, 'nothing to copy');
        const [mime, data] = formats[0];
        const source = Meta.SelectionSourceMemory.new(mime, new GLib.Bytes(data));
        this._selection.set_owner(Meta.SelectionType.SELECTION_CLIPBOARD, source);
    }

    PressKeysAsync([combo], invocation) {
        const keys = KEYS[combo];
        if (!keys) {
            invocation.return_dbus_error(`${NAME}.Error.BadKeys`, `unknown keys: ${combo}`);
            return;
        }
        // A shortcut's Super or Alt may still be held, which would turn the paste into another
        // shortcut: wait until they are released.
        const started = GLib.get_monotonic_time();
        const press = () => {
            const [, , mods] = global.get_pointer();
            const waited = (GLib.get_monotonic_time() - started) / 1000;
            if ((mods & HELD) && waited < RELEASE_WAIT_MS)
                return GLib.SOURCE_CONTINUE;
            this._releaseWait = 0;
            let time = GLib.get_monotonic_time();
            for (const key of keys)
                this._keyboard.notify_keyval(time++, key, Clutter.KeyState.PRESSED);
            for (const key of [...keys].reverse())
                this._keyboard.notify_keyval(time++, key, Clutter.KeyState.RELEASED);
            invocation.return_value(null);
            return GLib.SOURCE_REMOVE;
        };
        if (press() === GLib.SOURCE_CONTINUE)
            this._releaseWait = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 10, press);
    }

    // Centre clippo's window on the monitor under the pointer, keep it above, and focus it.
    // A Wayland window's app id arrives after 'window-created', so check on its first frame.
    _placeIfClippo(win) {
        const place = () => {
            if (win.get_wm_class() !== CLIPPO_APP_ID)
                return;
            const area = win.get_work_area_for_monitor(global.display.get_current_monitor());
            const frame = win.get_frame_rect();
            win.move_frame(true, area.x + Math.round((area.width - frame.width) / 2),
                area.y + Math.round((area.height - frame.height) / 2));
            win.make_above();
            win.activate(global.get_current_time());
        };
        const actor = win.get_compositor_private();
        if (actor)
            actor.connectObject('first-frame', place, this);
        else
            place();
    }
}
