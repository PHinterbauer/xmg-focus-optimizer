# Signing the executable

The release binary is unsigned. Windows will show a publisher warning, and Smart App
Control will refuse to run it outright. This file explains the options and what each one
actually costs.

## The constraint

There is no way around this in code. Windows trusts a binary because it carries an
Authenticode signature chaining to a certificate in the machine's trust store. A manifest,
a nice icon, a good reputation on GitHub — none of that substitutes. Someone has to issue a
certificate against a verified identity.

That part is not negotiable. Everything else is a choice of cost and hassle.

## The options

### 1. Do nothing, tell people to unblock it — free

What the release notes already say:

```powershell
Unblock-File -LiteralPath ".\XMG-Optimizer.exe"
```

Or, for Smart App Control (which has no exclusion list): disable SAC under
Windows Security → App & browser control → Smart App Control → Off. Smart App
Control is the strict one; the older "Windows protected your PC" warning is not.

Fine for yourself and a few people. Not fine for distribution.

#### Telling the two apart

They look like the same problem and they are not:

| Symptom | Mechanism | Fix |
|:--|:--|:--|
| *"Windows protected your PC"*, with a **More info** → **Run anyway** | Mark of the Web on the downloaded file | `Unblock-File` |
| Nothing happens. No dialog, no error | Smart App Control refusing an unsigned binary | folder exclusion |

`Unblock-File` removes the Zone.Identifier stream. It does **not** satisfy Smart App Control,
which judges the signature, not where the file came from.

#### Why this can break without any change to the binary

Smart App Control has three states: off, evaluation, on. **Evaluation logs but does not enforce.**
A binary can therefore run happily for weeks and then stop running when the setting moves to
enforcing — which happens without anyone touching it, or because the setting was changed.

The failure looks like a regression in the app. It is not. Every build in this repository is
unsigned, including the ones that used to run. To confirm which case you are in:

```
Event Viewer → Applications and Services Logs →
  Microsoft → Windows → Code Integrity → Operational
```

Events **3033** and **3077** naming `XMG-Optimizer.exe` with *"did not meet the Enterprise signing
level requirements"* are Smart App Control. They are logged in evaluation mode too, so their
presence alone does not prove enforcement — but paired with a binary that will not start, they are
the answer.

Policy ID `{0283AC0F-FFF1-49AE-ADA1-8A933130CAD6}` in those events is Microsoft's own shipped
policy, not anything configured on the machine.

#### Important: SAC has no exclusion list

Unlike Windows Defender SmartScreen, Smart App Control does **not** support exclusions. You cannot
add a folder or file to an allowlist. The only options are to disable SAC entirely or to sign the
binary. Any guidance suggesting otherwise is wrong.

### 2. Self-signed certificate, installed on each machine — free

You can create a certificate, sign the binary with it, and install it into **Trusted
Publishers** on your own machine. The publisher warning disappears there.

It does nothing for anyone else, and it does not satisfy Smart App Control, which judges
reputation rather than just chain validity. It is honest for a personal build and
pointless as a distribution strategy.

### 3. Microsoft Store, packaged as MSIX — free, but a different shape

Microsoft signs anything distributed through the Store, so you get a Microsoft-signed
binary with real reputation and no certificate to buy. Partner Center is free for
individual developers.

The cost is that it is not a raw `.exe` any more. It becomes an MSIX package, installed
through the Store, and it goes through Store review. This is the only free route to
"Windows does not flag it" for people who are not you.

### 4. A commercial code-signing certificate — the real answer

| Provider | Price | Validation |
|:--|:--|:--|
| DigiCert OV | from ~$696/yr | **Organisation** |
| DigiCert EV | from ~$972/yr | **Organisation** (needed only for drivers) |
| Azure Artifact Signing | much cheaper, Basic/Public Trust SKU | **Organisation** |

Two things to know before you spend anything:

- **There is no individual option at the mainstream providers.** DigiCert's page says
  "verification of your organization". You would need a registered business entity —
  for a sole trader that is usually a few days and a modest fee, depending on where you
  are.
- **Since June 2023 the CA/Browser Forum requires the private key to live in hardware** —
  an HSM, or the CA's cloud equivalent such as DigiCert KeyLocker. A plain `.pfx` exported
  to disk is no longer issuable from most CAs. This is why route B below passes a password:
  on the newer setups there often isn't one.

For a personal, non-commercial tool the honest summary is: **this costs real money and
real identity verification to solve a problem that only exists for other people.** If you
are the only user, option 1 is the right answer.

## Setting it up

`sign-release.ps1` handles all of it and is already wired into CI. It supports a `.pfx` or
Azure Artifact Signing, and it **verifies the signature after signing** rather than trusting
the exit code.

### Route A — a `.pfx`

Add these as repository secrets:

| Secret | Value |
|:--|:--|
| `XMG_SIGN_PFX_PATH` | path to the `.pfx` on the runner |
| `XMG_SIGN_PFX_PASSWORD` | its password |

Note the `PATH` in the name means *a path on the runner*. If your certificate is stored in
the Windows certificate store instead, use `signtool sign /a` and drop the `/f` flag.

### Route B — Azure Artifact Signing

| Secret | Value |
|:--|:--|
| `XMG_SIGN_AZURE_ENDPOINT` | `https://weu.codesigning.azure.net` |
| `XMG_SIGN_AZURE_CERT_PROFILE` | your certificate profile name |

The private key never leaves Microsoft's HSM. This is the better option of the two if you
go this route.

## What CI does

`build.yml` signs the executable after linking **only if** the secrets are present, then
re-runs the resource check on the signed file. An unsigned build still succeeds, so local
development is unaffected.

## Checking a signed build yourself

`Verify-Release.ps1` reports the resources and the hash. For the signature:

```powershell
Get-AuthenticodeSignature .\XMG-Optimizer.exe | Format-List Status, SignerCertificate
```

`Status` must be `Valid`. Anything else — `NotSigned`, `UnknownError`, `HashMismatch` — means
do not run it.