<div align="center">

# XMG Focus Optimizer

**Two safe performance modes for the SCHENKER XMG FOCUS 15.**
Native Windows app, written in Rust. No PowerShell, no console window.

`Quiet` for cool and quiet · `Performance` for full speed · both adapt when you unplug

[![Build](https://github.com/PHinterbauer/xmg-focus-optimizer/actions/workflows/build.yml/badge.svg)](https://github.com/PHinterbauer/xmg-focus-optimizer/actions/workflows/build.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

## What it does

| | |
|---|---|
| **Quiet** | CPU capped at 70%, Windows biased toward the integrated GPU, RTX throttled to 25 W, fans quiet |
| **Performance** | Full boost, every power saving dropped, GPU at its 85 W maximum, fans on turbo |
| **Automatic** | Unplug the charger and the battery values are applied. Plug it back in and mains returns. |
| **Persistent** | Re-launches at sign-in and quietly restores whichever mode you had selected. |

Verified across all four mains/battery combinations on the target machine.

---

## Safety is the design

Every change this app makes is reversible, and a way back exists **before** each one.

**It never:**

- overclocks or undervolts
- disables HVCI, VBS, Credential Guard or any other security feature
- touches the BIOS or flashes firmware
- writes undocumented EC or driver binaries — `nvdrsdb0.bin`, `SetDCHU_Data` in `InsydeDCHU.dll`
  *(its parameters are unpublished, and a wrong call can leave a laptop unbootable)*
- deletes a power scheme it did not create
- changes a setting you asked it to keep

**What it writes** — 16 standard Windows power settings through `powercfg`, the NVIDIA power
limit via `nvidia-smi`, and a few `HKCU` registry values. Every power setting is **read back and
verified** after being written.

Fans are driven through **UI Automation of the manufacturer's own app** — the same buttons you
would click yourself. Reaching the fan controller directly means undocumented firmware structures.

The GPU wattage is range-checked *and* compared against the card's own reported maximum before it
is sent, so a faulty driver reading cannot push the hardware past what it supports.

---

## Backups can only be added, never removed

```
XMG-Backup/
├── original-settings.txt          ← the initial backup, kept forever
├── original-autostart.txt
├── original-gaming.json
├── original-gpu-limit.txt
├── snapshots/                     ← before every single change
└── initial-archive/
    └── initial-20261013-141500/   ← the previous initial backup, kept forever
```

- An **initial backup** is captured before the first change and never touched again.
- A **snapshot** is taken before *every* change — applying Quiet, applying Performance, and
  restoring alike. A restore therefore also snapshots what it is replacing, so it can be undone.
- Creating a **new** initial backup copies the existing one into `initial-archive/<timestamp>/`
  first and verifies the copy byte-for-byte. If the archive copy fails, nothing is overwritten and
  the action fails loudly.
- Manual snapshots are never trimmed. Only automatic ones are, past 40.

An existing initial backup from the earlier command-line tool is detected and reused rather than
replaced, so upgrading never starts from a blank slate.

---

## Consistency check

The Dashboard compares **all 16 settings** against what the selected mode should produce for the
current power source. When something disagrees, the app changes nothing until you choose:

- **Change to Quiet / Performance** — rewrite the settings to match the mode
- **Keep current settings** — leave the machine exactly as it is

> **Why this is not noisy.** A power scheme stores a separate AC and DC column, and this app writes
> the values for the *current* source into **both**. So after a normal apply on mains the scheme
> legitimately holds the mains numbers even once you move to battery. A setting therefore counts as
> consistent when it matches the current source's values **or** the other source's. Only a value
> matching neither — an outside change, a different tool, a hand-edited scheme — is reported.

**Re-applying does nothing when nothing is wrong.** Applying runs the check first; if the active
scheme already belongs to the app and every setting matches, the scheme is left completely
untouched. Startup entries, GPU limit and fans are still ensured, because those are not stored in
the scheme.

---

## Install

1. Download `XMG-Optimizer.exe` from the Actions artefacts or Releases.
2. Right-click → **Run as administrator**. This one elevated run registers a background task.
3. Accept the prompt. From then on, applying a mode needs **no** further prompts.

> If Smart App Control refuses a freshly built unsigned binary, add the folder under
> Windows Security → App & browser control → Exclusions.

Closing the window collapses it into the notification area so the background watcher keeps running.
Quit from the tray menu or the Settings tab.

### Command line

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

---

## Files

| Purpose | Location |
|---|---|
| Backups, snapshots, archive | next to the executable, in `XMG-Backup\` |
| Settings and logs | `%LOCALAPPDATA%\XMG-Optimizer\` |

---

## Build from source

```sh
cargo build --release
```

Needs the MSVC toolchain, for `rc.exe`, which embeds the icon and manifest. The build also works
with the GNU toolchain using MinGW's `windres`. CI runs on every push.

---

## Known limitation

The RTX 3050 Ti idles in performance state P0 — roughly 16 W instead of 0.5 W. This is a driver
profile held in an undocumented binary, so it is **not** automated. Change it by hand in
NVIDIA Control Panel → *Manage 3D settings* → *Power management mode* → **Prefer maximum
performance**.

---

## Credits

The power-setting GUIDs and both mode profiles were established empirically against the target
machine with `powercfg /qh`, then verified across all four mains/battery combinations.

<div align="center">
<sub>MIT licensed · Built for the SCHENKER XMG FOCUS 15 · Not affiliated with SCHENKER or NVIDIA</sub>
</div>