<div align="center">

# 𝗫𝗠𝗚&nbsp;𝗙𝗼𝗰𝘂𝘀&nbsp;𝗢𝗽𝘁𝗶𝗺𝗶𝘇𝗲𝗿

### Two safe performance modes for the SCHENKER XMG FOCUS 15

*Quiet when you want it cool. Performance when you want it fast. Both adjust by themselves when you unplug the charger.*

<br>

[![Build](https://github.com/PHinterbauer/xmg-focus-optimizer/actions/workflows/build.yml/badge.svg)](https://github.com/PHinterbauer/xmg-focus-optimizer/actions/workflows/build.yml)
[![Tests](https://img.shields.io/badge/tests-67%20passing-22a06b)](src/tests.rs)
[![Release](https://img.shields.io/github/v/release/PHinterbauer/xmg-focus-optimizer?label=release)](https://github.com/PHinterbauer/xmg-focus-optimizer/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-5b8def)](LICENSE)

[**Download the latest release**](https://github.com/PHinterbauer/xmg-focus-optimizer/releases/latest) &nbsp;&middot;&nbsp; [**Report an issue**](https://github.com/PHinterbauer/xmg-focus-optimizer/issues)

</div>

---

## What it does

| Mode | Behaviour |
|:--|:--|
| **Quiet** | CPU capped at 70%, Windows biased toward the integrated GPU, RTX throttled to 25 W, fans kept quiet, display at 60 Hz and 70% brightness |
| **Performance** | Full boost, every power saving dropped, GPU at its 85 W maximum, fans on turbo, display at max refresh rate and 100% brightness |
| **Automatic** | Unplug the charger and the battery values are applied. Plug it back in and mains returns. |
| **Persistent** | Registers *itself* to come back at sign-in and quietly restores whichever mode you had selected. |

Verified across all four mains/battery combinations on the target machine.

---

## Safety is the design

Every change this app makes is reversible, and a way back exists **before** each one.

**It never:**

- overclocks or undervolts
- disables HVCI, VBS, Credential Guard or any other security feature
- touches the BIOS or flashes firmware
- writes undocumented EC or driver binaries &mdash; `nvdrsdb0.bin`, `SetDCHU_Data` in `InsydeDCHU.dll`
  <br><sub>*its parameters are unpublished, and a wrong call can leave a laptop unbootable*</sub>
- deletes a power scheme it did not create
- changes a setting you asked it to keep
- adds, removes, delays or rewrites **another program's startup entry**

> [!NOTE]
> The one startup entry the app writes is **its own**, `XMGOptimizer`, so it can come back at sign-in.
> That entry is added and removed only by the *Start with Windows* checkbox. The per-entry
> enabled/disabled flags Task Manager uses are never read or written, so what each program does at
> sign-in stays exactly as you arranged it.

**What it writes** &mdash; 16 standard Windows power settings through `powercfg`, the NVIDIA power
limit via `nvidia-smi`, and a few `HKCU` registry values. Every power setting is **read back and
verified** after being written.

Fans are driven through **UI Automation of the manufacturer's own app** &mdash; the same buttons you
would click yourself. Reaching the fan controller directly means undocumented firmware structures.

The GPU wattage is range-checked *and* compared against the card's own reported maximum before it
is sent, so a faulty driver reading cannot push the hardware past what it supports.

> [!IMPORTANT]
> There is exactly one thing this app will not do for you: the RTX 3050 Ti idles in performance
> state P0, and that lives in a driver profile in an undocumented binary. See *Known limitation*.

---

## Backups can only be added, never removed

```
XMG-Backup/
├── original-settings.txt          ← the initial backup, kept forever
├── original-autostart.txt            ← a read-only record of your sign-in; never written back
├── original-gaming.json
├── original-gpu-limit.txt
├── snapshots/                     ← before every single change
└── initial-archive/
    └── initial-20261013-141500/   ← the previous initial backup, kept forever
```

- An **initial backup** is captured before the first change and never touched again.
- A **snapshot** is taken before *every* change &mdash; applying Quiet, applying Performance, and
  restoring alike. A restore therefore also snapshots what it is replacing, so it can be undone.
- Creating a **new** initial backup copies the existing one into `initial-archive/<timestamp>/`
  first and verifies the copy byte-for-byte. If the archive copy fails, nothing is overwritten and
  the action fails loudly.
- Manual snapshots are never trimmed. Only automatic ones are, past 40.
- Making a **new** initial backup requires three separate confirmations.

> [!TIP]
> An initial backup from the earlier command-line tool is detected and reused rather than replaced,
> so upgrading from it never starts from a blank slate.

---

## Consistency check

The Dashboard compares **all 17 power settings plus the display refresh rate** against what the selected mode should produce for the
current power source. When something disagrees, the app changes nothing until you choose:

- **Change to Quiet / Performance** &mdash; rewrite the settings to match the mode
- **Keep current settings** &mdash; leave the machine exactly as it is

> [!NOTE]
> **Why this is not noisy.** A power scheme stores a separate AC and DC column, and this app writes
> the values for the *current* source into **both**. So after a normal apply on mains the scheme
> legitimately holds the mains numbers even once you move to battery. A setting therefore counts as
> consistent when it matches the current source's values **or** the other source's. Only a value
> matching neither &mdash; an outside change, a different tool, a hand-edited scheme &mdash; is
> reported.

**Re-applying does nothing when nothing is wrong.** Applying runs the check first; if the active
scheme already belongs to the app and every setting matches, the scheme is left completely
untouched. The GPU limit and fans are still ensured, because those are not stored in the scheme.

---

## History

Every change is a snapshot, so the record is complete by construction. The **History** tab turns
that record into something you can actually read:

- **Filter** &mdash; all, automatic only, or manual only
- **Search** &mdash; over time, event, mode and snapshot id
- **Details** &mdash; opens one entry: what it was, which mode, how many settings it captured, its
  file &mdash; and the full list of settings that moved, before &rarr; after
- **A / B** &mdash; mark any two entries, not just neighbours, and see exactly how many settings
  differ between them
- **File** &mdash; shows the snapshot in Explorer
- **Restore** &mdash; the one button here that changes anything

> [!NOTE]
> **History deletes nothing.** Filters, search and the comparison are all views over the snapshots
> that already exist. Backups stay append-only, and the trim past 40 applies to automatic snapshots
> only &mdash; a manual snapshot is never removed.

---

## Install

1. Download `XMG-Optimizer.exe` from the [latest release](https://github.com/PHinterbauer/xmg-focus-optimizer/releases/latest).
2. Remove the download mark, otherwise Windows refuses to run it:
   ```powershell
   Unblock-File -LiteralPath "$env:USERPROFILE\Downloads\XMG-Optimizer.exe"
   ```
3. Right-click &rarr; **Run as administrator**. This one elevated run registers a background task.
4. Accept the prompt. From then on, applying a mode needs **no** further prompts.

> [!TIP]
> Closing the window collapses it into the notification area, so the background watcher keeps
> reacting when you unplug. Quit from the tray menu or the Settings tab.

### If Windows still refuses to run it

The binary is **unsigned**, so two different Windows checks can stop it. They need
different fixes:

| What you see | Cause | Fix |
|:--|:--|:--|
| *"Windows protected your PC"* &rarr; **More info** &rarr; **Run anyway** | the file is marked as downloaded | `Unblock-File`, as above |
| Nothing happens at all, no dialog | **Smart App Control** blocks unsigned binaries outright | disable SAC (see below) |

For the second case: **Windows Security → App & browser control → Smart App
Control → Off**.

> [!IMPORTANT]
> **Smart App Control has no exclusion list.** Unlike Windows Defender
> SmartScreen, you cannot add a folder or file to an allowlist. The only options
> are to disable SAC or to sign the binary.
>
> **If the app ran for you before and does not now, it is almost certainly this.**
> Nothing about the binary changed. Smart App Control begins in a non-enforcing
> *evaluation* mode and can switch to enforcing on its own, so a binary that ran
> last week can be refused today with no change to the file.
>
> To confirm, look at `Microsoft-Windows-CodeIntegrity/Operational` in Event
> Viewer. Events **3033** and **3077** reading *"did not meet the Enterprise
> signing level requirements"* for `XMG-Optimizer.exe` mean Smart App Control
> blocked it.
>
> Disabling SAC is a one-time, permanent change (you cannot easily re-enable it
> afterwards). The alternative is code signing — see [SIGNING.md](SIGNING.md).

<details>
<summary><b>Command line reference</b></summary>

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

</details>

---

## Build from source

```sh
cargo build --release
```

Needs the MSVC toolchain, for `rc.exe`, which embeds the icon and manifest. The build also works
with the GNU toolchain using MinGW's `windres`.

CI runs `cargo check`, the unit tests, `clippy` and a release build on every push, and attaches the
executable as an artefact.

<details>
<summary><b>What the tests cover</b></summary>

`src/tests.rs` &mdash; the parts that must never silently go wrong:

- Quiet is strictly more conservative than Performance, on every axis
- Battery is never more aggressive than mains
- All 17 GUIDs are well formed and unique; the two fixed pairs keep their AC/DC split
- Backup parsing rejects traversal, non-numeric and malformed-GUID input
- Restore sources refuse `../` style path escapes
- The AC/DC tolerance that stops false drift warnings
- The scheduled task passes the battery-start check that charger auto-switch depends on
- History: filters are exact, a diff lists only what moved, a vanished setting reads as a dash
- Temperature parsers reject Celsius reported where tenths of Kelvin are expected

</details>

---

## Known limitation

The RTX 3050 Ti idles in performance state P0 &mdash; roughly 16 W instead of 0.5 W. This is a
driver profile held in an undocumented binary, so it is **not** automated. Change it by hand in
NVIDIA Control Panel &rarr; *Manage 3D settings* &rarr; *Power management mode* &rarr;
**Prefer maximum performance**.

### CPU temperature

The header always shows a CPU and a GPU reading. A sensor with no data shows as `CPU &ndash;`,
never as a missing pill &mdash; so you can tell "no reading" apart from "not being measured". The
value is never invented.

Windows exposes no CPU temperature API, so the app tries documented sources in order:
LibreHardwareMonitor, OpenHardwareMonitor, the firmware `MSAcpi` thermal zone, then the thermal-zone
performance counter. **If your firmware publishes none of them, the reading stays `&ndash;`.**

> [!NOTE]
> Running [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor)
> fills the reading in immediately &mdash; the app picks it up on its next poll, no restart.
>
> Reading the embedded controller directly is deliberately **not** done. Those interfaces
> (`WinRing0`, `SetDCHU_Data`, `nvdrsdb0.bin`) are undocumented, and a wrong call can leave a
> laptop unbootable. A dash in the header is the honest answer; an unbootable laptop is not an
> acceptable price for it.

---

## Where files live

| Purpose | Location |
|:--|:--|
| Backups, snapshots, archive | next to the executable, in `XMG-Backup\` |
| Settings and logs | `%LOCALAPPDATA%\XMG-Optimizer\` |

---

## Credits

The power-setting GUIDs and both mode profiles were established empirically against the target
machine with `powercfg /qh`, then verified across all four mains/battery combinations.

<div align="center">
<sub>MIT licensed &middot; Built for the SCHENKER XMG FOCUS 15 &middot; Not affiliated with SCHENKER or NVIDIA</sub>
</div>
