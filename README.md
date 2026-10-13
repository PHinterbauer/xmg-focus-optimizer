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
| **Dashboard** | Pick Quiet or Performance, apply, and see the consistency check |
| **Backups** | Take snapshots, restore from the initial backup, a snapshot, or an archived backup |
| **Settings** | Start with Windows, collapse to tray, automatic re-apply on power change, theme |

Creating a new initial backup takes **three deliberate confirmations** and never overwrites the
previous one.

Closing the window collapses it into the notification area so the background power watcher keeps
running. Quit from the tray menu or the Settings tab.

### Consistency check

The Dashboard compares **every one of the 16 settings** against what the selected mode should
produce for the current power source, and tells you when they disagree. Expand **Show all settings**
to see expected versus actual for each.

When something differs you get a choice, and the app changes nothing until you pick:

- **Change to Quiet / Performance** — rewrite the settings to match the mode
- **Keep current settings** — leave the machine exactly as it is

The check also notices when the active power scheme is not one this app created, which is usually
what happens after another tool or a manual change has taken over.

> **One subtlety.** A power scheme stores a separate AC and DC column, and this app writes the
> values for the *current* power source into **both**. So after a normal apply on mains, the scheme
> legitimately holds the mains numbers even once you move to battery. Counting that as drift would
> mean a permanent false alarm, so a setting is treated as consistent when it matches the current
> source's values **or** the other source's. Only a value matching neither — an outside change, a
> different tool, a hand-edited scheme — is reported.

### Re-applying does nothing when nothing is wrong

Applying first runs the consistency check. If the active scheme already belongs to this app and
every setting matches, the scheme is left completely untouched — no rewrite, no new scheme. Startup
entries, GPU limit and fans are still ensured, because those are not stored in the scheme.

### Surviving a restart

The selected mode is saved the moment you pick it, so it is still there after a reboot.

With **Start with Windows** on (the default, and it is registered in the registry on first run),
the app launches from the Run key, notices it was started by Windows, and silently re-applies the
saved mode. Since applying is idempotent, this normally changes nothing at all — it just guarantees
the laptop came up the way you left it. A snapshot is still recorded each time, so there is a
record of the state at sign-in.

The Run key is reconciled against the stored preference on every launch, so the checkbox in
Settings and the actual registry entry can never drift apart.

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