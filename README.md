# XMG Focus Optimizer

Two safe performance modes for the **SCHENKER XMG FOCUS 14 (Mid 2021)** — i5-11400H / RTX 3050 Ti.

One switch between **Quiet** (cool, silent, long-lasting) and **Performance** (full power, full speed).
Both modes adapt on their own when you unplug the charger.

Native Windows GUI, written in Rust. No PowerShell, no console window.

---

## Safety is the design, not an afterthought

Everything this app does is reversible, and a way back exists before every single change.

**It never does any of the following:**

- No overclocking, no undervolting, no turbo beyond what the hardware already offers
- No disabling HVCI / VBS, Credential Guard or any other Windows security feature
- No BIOS edits, no firmware flashing
- No writes to undocumented EC or driver binaries (`nvdrsdb0.bin`, `SetDCHU_Data` in `InsydeDCHU.dll`)
  — the parameter meanings of that call are not published, and a wrong call can leave a laptop unable to boot
- No deletion of power schemes it did not create; it only ever removes schemes carrying its own name prefix

**What it writes** — 16 standard Windows power settings via `powercfg`, the NVIDIA power limit via
`nvidia-smi` (range-checked *and* capped against the device's own reported maximum), and a handful of
`HKCU` registry values. Every power setting is read back after writing and verified.

Fans are driven through **UI Automation of the manufacturer's own app** — the same buttons you would
click yourself. That is deliberate: reaching the fan controller directly means undocumented firmware
structures.

### Backups can only be added, never removed

- An **initial backup** is captured before the first change, and kept forever.
- A **snapshot** is taken automatically before *every* change — applying Quiet, applying Performance,
  and restoring alike.
- Creating a **new** initial backup copies the existing one into `initial-archive/<timestamp>/` first,
  and verifies the copy byte-for-byte. If the archive copy fails, nothing is overwritten and the
  action fails loudly.
- Manual snapshots are never trimmed. Only automatic ones are, past 40.

Restoring always works, and a restore takes its own snapshot first — so a restore can itself be undone.

An existing initial backup from the previous command-line tool is detected and reused rather than
replaced, so upgrading never starts from a blank slate.

---

## Installing

1. Download `XMG-Optimizer.exe` from the Actions artefacts (or Releases).
2. Right-click it → **Run as administrator**. This one elevated run registers a background task.
3. Accept the prompt. From then on, applying a mode needs **no** further prompts.

> If Smart App Control is enabled on your machine and refuses to run a freshly built unsigned binary,
> add the folder as an exclusion in Windows Security → App & browser control → Exclusions.

## Using it

| Where | What |
|---|---|
| **Dashboard** | Pick Quiet or Performance, apply, and see what is currently active |
| **Backups** | Take snapshots, restore from the initial backup, a snapshot, or an archived backup |
| **Settings** | Start with Windows, collapse to tray, automatic re-apply on power change, theme |

Creating a new initial backup takes **three deliberate confirmations** and never overwrites the
previous one.

Closing the window collapses it into the notification area so the background power watcher keeps
running. Quit from the tray menu or the Settings tab.

### Background behaviour

With **Re-apply automatically when the charger changes** enabled, the tray icon watches the power
source. Unplugging the charger re-applies the battery values; plugging it back in restores the mains
values. No notification, no window.

## Command line

```
XMG-Optimizer.exe                     open the window
XMG-Optimizer.exe --tray              start hidden in the tray
XMG-Optimizer.exe apply quiet         apply a mode (quiet | performance)
XMG-Optimizer.exe apply performance
XMG-Optimizer.exe restore             restore the initial backup
XMG-Optimizer.exe restore snapshot    restore the most recent snapshot
XMG-Optimizer.exe snapshot            save a snapshot now
XMG-Optimizer.exe factory             archive the current backup, make a new initial one
XMG-Optimizer.exe help
```

## Where files live

| Purpose | Location |
|---|---|
| Backups, snapshots, archive | next to the executable, in `XMG-Backup\` (or an existing backup folder if one is found) |
| Settings, logs | `%LOCALAPPDATA%\XMG-Optimizer\` |

An existing `XMG-Backup` folder containing `original-settings.txt` is always preferred, so the
backup from the earlier command-line tool keeps being found and used.

## Known limitation

The RTX 3050 Ti idles in performance state P0 (~16 W instead of ~0.5 W). This is a driver-profile
setting held in an undocumented binary, so it is **not** automated. It can be changed by hand in
NVIDIA Control Panel → *Manage 3D settings* → *Power management mode* → **Prefer maximum
performance**.

## Building from source

```sh
cargo build --release
```

Requires the MSVC toolchain (for `rc.exe`, which embeds the icon and manifest). The build also
works with the GNU toolchain, using MinGW's `windres`.

The CI workflow runs `cargo fmt`, `clippy` and a release build on every push.

## Credits

The power-setting GUIDs and both mode profiles were established empirically against this specific
machine with `powercfg /qh`, then verified across all four mains/battery combinations.

## License

MIT — see [LICENSE](LICENSE).