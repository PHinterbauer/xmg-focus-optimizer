# Clean factory reference

`original-settings.txt` holds the **16 power settings this app manages**, as
`subgroup|setting|ac|dc`, captured from the original machine backup.

## What is in here, and what is deliberately not

The app's `profile::DEFS` table lists exactly 16 setting GUIDs. This file
contains one row per GUID, nothing else — filtered out of the 178-row
`original-settings.txt` that `powercfg /qh` produces for a full scheme.

Each row is two GUIDs and two small integers: a standard Windows power-subgroup
GUID, a standard setting GUID, and the AC/DC values. There are no usernames, no
paths, no hostnames, no registry values, no GPU data. The full backup also
contains `original-autostart.txt` (program paths with the Windows username in
them), per-program `startup-*.txt` files, an energy report and a log — **none
of that is committed, and none of it ever should be**.

## What it is for

A human-readable reference for "what did stock look like". The app never reads
this file at runtime — it uses the live initial backup in `XMG-Backup/` — so
editing it changes nothing about the program.
