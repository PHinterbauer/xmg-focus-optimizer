# =============================================================================
#  Verify a downloaded XMG-Optimizer.exe before running it.
# =============================================================================
#  Checks the things that have actually gone wrong in this project before:
#
#    1. The executable has a real multi-size icon embedded.
#    2. The Windows manifest is embedded. Without it the taskbar and Start-menu
#       identity are wrong.
#    3. The file's version metadata is populated.
#    4. The app has NOT registered itself in startup.
#
#    Plus a SHA256, so you can confirm the bytes match the release.
#
#  Usage:
#    pwsh -File Verify-Release.ps1 -Path "$env:USERPROFILE\Downloads\XMG-Optimizer.exe"
#
#  This script only reads the file. It does not run it.
# =============================================================================

param(
    [Parameter(Mandatory = $true)]
    [string]$Path
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

if (-not (Test-Path $Path)) { throw "not found: $Path" }
$full = (Resolve-Path $Path).Path
$fi   = Get-Item $full

Write-Host ""
Write-Host "  XMG-Optimizer.exe - pre-flight check"
Write-Host "  ------------------------------------"
Write-Host ("  file     : {0}" -f $full)
Write-Host ("  size     : {0:N2} MB" -f ($fi.Length / 1MB))
Write-Host ("  modified : {0}" -f $fi.LastWriteTime)
Write-Host ("  sha256   : {0}" -f (Get-FileHash $full -Algorithm SHA256).Hash)
Write-Host ""

# --- mark of the web -----------------------------------------------------
$zone = Get-Content -Path $fi.FullName -Stream Zone.Identifier -ErrorAction SilentlyContinue
if ($zone) {
    Write-Host "  [!] Mark-of-the-Web is present, so Windows will refuse to run it."
    Write-Host "      Once you have checked the SHA256 above, run:"
    Write-Host ('         Unblock-File -LiteralPath "' + $full + '"')
    Write-Host ""
} else {
    Write-Host "  [ok] No mark-of-the-web (already unblocked)"
    Write-Host ""
}

# --- 1. embedded resources (icon, manifest, version) ---------------------
# The PE resource directory is walked directly. Do not use System.Drawing.Icon
# on an .exe: it does not report the embedded frames and reports nothing at all,
# which reads as "icon missing" on a perfectly good build.
$bytes = [System.IO.File]::ReadAllBytes($full)
function U16([int]$o) { [int][BitConverter]::ToUInt16($bytes, $o) }
function U32([int]$o) { [uint32][BitConverter]::ToUInt32($bytes, $o) }
function Low31([uint32]$v) { [int]($v -band [uint32]0x7FFFFFFF) }

$peOff = [int](U32 0x3C)
$coff = $peOff + 4
$nSec = U16 ($coff + 2)
$optSz = U16 ($coff + 16)
$opt = $coff + 20
$ddOff = if ((U16 $opt) -eq 0x20B) { $opt + 112 } else { $opt + 96 }
$resRva = U32 ($ddOff + 16)            # entry 2 = resource table
$script:secOff = $opt + $optSz
$script:nSec = $nSec

function R2O([uint32]$rva) {
    for ($j = 0; $j -lt $script:nSec; $j++) {
        $o = $script:secOff + 40 * $j
        $va = U32 ($o + 12); $vs = U32 ($o + 8)
        if ($rva -ge $va -and $rva -lt ($va + $vs)) { return [int]((U32 ($o + 20)) + ($rva - $va)) }
    }
    return -1
}

$res = @{}
$resBase = R2O $resRva
if ($resBase -lt 0) {
    Write-Host "  [!!] No resource directory - the build embedded nothing."
} else {
    $names = @{ 3 = 'RT_ICON'; 14 = 'RT_GROUP_ICON'; 16 = 'RT_VERSION'; 24 = 'RT_MANIFEST' }
    $nNamed = U16 ($resBase + 12); $nId = U16 ($resBase + 14)
    for ($i = 0; $i -lt ($nNamed + $nId); $i++) {
        $e = $resBase + 16 + 8 * $i
        $id = [int](U32 $e)
        $sub = $resBase + (Low31 (U32 ($e + 4)))
        $res[$names[$id]] = (U16 ($sub + 12)) + (U16 ($sub + 14))
    }
    $iconCount = if ($res.ContainsKey('RT_ICON')) { $res['RT_ICON'] } else { 0 }
    if ($res.ContainsKey('RT_GROUP_ICON') -and $iconCount -ge 5) {
        Write-Host ("  [ok] Icon embedded: {0} sizes" -f $iconCount)
    } else {
        Write-Host ("  [!!] Icon missing or too small - {0} frame(s), group icon {1}" -f `
                    $iconCount, $(if ($res.ContainsKey('RT_GROUP_ICON')) { 'present' } else { 'MISSING' }))
    }
    foreach ($want in 'RT_GROUP_ICON', 'RT_MANIFEST', 'RT_VERSION') {
        if ($res.ContainsKey($want)) {
            Write-Host ("  [ok] {0} embedded" -f $want)
        } else {
            Write-Host ("  [!!] {0} MISSING" -f $want)
        }
    }
}

# --- 2. manifest content ------------------------------------------------
$ascii = [System.Text.Encoding]::ASCII.GetString($bytes)
if ($ascii.Contains('requestedExecutionLevel') -and $ascii.Contains('uiAccess')) {
    Write-Host "  [ok] Manifest content present (requestedExecutionLevel)"
} else {
    Write-Host "  [!!] Manifest content not found."
}

# --- 3. version metadata -------------------------------------------------
$vi = $fi.VersionInfo
if ($vi.FileDescription -and $vi.ProductName) {
    Write-Host ("  [ok] Version info: {0} - {1} ({2})" -f $vi.ProductName, $vi.FileVersion, $vi.FileDescription)
} else {
    Write-Host "  [!!] Version metadata looks empty."
}

# --- 4. it must not be in startup yet ------------------------------------
$runKey   = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run'
$startup  = [Environment]::GetFolderPath('Startup')
$inRun    = $null
if (Test-Path $runKey) {
    $inRun = (Get-ItemProperty $runKey).PSObject.Properties |
             Where-Object { $_.Name -notlike 'PS*' -and ($_.Name -like '*XMG*' -or [string]$_.Value -like '*XMG*') }
}
$inStartup = @(Get-ChildItem $startup -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -like '*XMG*' })
if (-not $inRun -and $inStartup.Count -eq 0) {
    Write-Host "  [ok] Not registered in startup. It does nothing until you run it."
} else {
    Write-Host "  [!] Startup entries exist:"
    if ($inRun)     { $inRun     | ForEach-Object { Write-Host ("        Run key : " + $_.Name) } }
    if ($inStartup) { $inStartup | ForEach-Object { Write-Host ("        Startup : " + $_.Name) } }
}

Write-Host ""
Write-Host "  First run:"
Write-Host "    1. Right-click the .exe > Run as administrator. One UAC prompt,"
Write-Host "       which registers a background task so later applies need none."
Write-Host "    2. Opening the window changes nothing. Settings are written only"
Write-Host "       when you click a mode, or plug the charger in and out with"
Write-Host "       auto-switch on."
Write-Host "    3. Undo everything: Settings > Backups > Restore from initial backup."
Write-Host ""