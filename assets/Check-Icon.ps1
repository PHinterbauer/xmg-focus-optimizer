# =============================================================================
#  Checks that src/icon.rs and assets/app.ico still draw the same icon.
#
#  The two are maintained by hand - the .ico by assets/New-Icon.ps1, the tray
#  and window icons by the Rust drawing code - and nothing at build time forces
#  them to agree. They have already drifted once: the Rust plate used
#  distance-to-the-nearer-edge where a superellipse needs distance-from-centre,
#  so every pixel was rejected and the runtime icon rendered completely blank.
#
#  This re-implements the Rust maths in PowerShell and compares the result with
#  the committed .ico. It is the local stand-in for `cargo test`, which does the
#  same thing in src/icon.rs - run that in CI, this when iterating.
#
#    pwsh -File assets/Check-Icon.ps1 -Size 64
# =============================================================================
param(
    [string]$IcoPath   = (Join-Path $PSScriptRoot 'app.ico'),
    # 64 px is the smallest frame where the two rasterisers settle down enough
    # for the tolerance below to mean something. At 16 px an 8x8 block is two
    # pixels across and the result is mostly noise.
    [int]$Size         = 64,
    [int]$Tolerance    = 26,
    [int]$Blocks       = 8
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

# --- constants, mirrored from src/icon.rs ------------------------------------
$PLATE      = @(0, 230, 118)
$GLYPH      = @(17, 17, 20)
$RING       = 0.205
$STROKE     = 0.090
$ARC_START  = -45.0
$ARC_SWEEP  = 270.0

$s   = [single]$Size
$pad = [single]($s * 0.078)
$w   = [single]($s - 2 * $pad)
$rad = [single]($w / 2.0)
$cx  = [single]($pad + $w / 2.0)
$cy  = [single]($pad + $w / 2.0)

$px    = [float[]]::new($Size * $Size * 4)
$cover = [float[]]::new($Size * $Size)

function Disc([single]$dcx, [single]$dcy, [single]$r) {
    $ri  = [int][Math]::Ceiling($r + 1.0)
    $cxi = [int][Math]::Round($dcx)
    $cyi = [int][Math]::Round($dcy)
    for ($y = $cyi - $ri; $y -le $cyi + $ri; $y++) {
        if ($y -lt 0 -or $y -ge $Size) { continue }
        for ($x = $cxi - $ri; $x -le $cxi + $ri; $x++) {
            if ($x -lt 0 -or $x -ge $Size) { continue }
            $dx = $x - $dcx; $dy = $y - $dcy
            $d  = [Math]::Sqrt($dx * $dx + $dy * $dy) - $r
            $a  = [Math]::Min([Math]::Max(0.5 - $d, 0.0), 1.0)
            if ($a -le 0.0) { continue }
            $i = $y * $Size + $x
            if ($a -gt $cover[$i]) { $cover[$i] = [single]$a }
        }
    }
}

# --- plate: superellipse about the centre ------------------------------------
for ($y = 0; $y -lt $Size; $y++) {
    for ($x = 0; $x -lt $Size; $x++) {
        $fx = $x + 0.5; $fy = $y + 0.5
        $dx = [Math]::Abs($fx - $cx)
        $dy = [Math]::Abs($fy - $cy)
        $q  = [Math]::Pow($dx / $rad, 4) + [Math]::Pow($dy / $rad, 4)
        if ($q -gt 1.06) { continue }
        $i = ($y * $Size + $x) * 4
        $px[$i]     = $PLATE[0]
        $px[$i + 1] = $PLATE[1]
        $px[$i + 2] = $PLATE[2]
        $px[$i + 3] = [Math]::Floor([Math]::Min([Math]::Max((1.06 - $q) / 0.06, 0.0), 1.0) * 255.0)
    }
}

# --- glyph: coverage as a maximum, composited once ----------------------------
# Stamping straight onto the colour buffer compounds wherever discs overlap and
# makes the stroke fatter and brighter than GDI+ draws the same path.
$rr = [single]($w * $RING)
$sw = [single]($w * $STROKE)
$steps = [Math]::Min([Math]::Max($Size * 8, 180), 1200)
for ($i = 0; $i -le $steps; $i++) {
    $deg = ($ARC_START + $ARC_SWEEP * ($i / $steps)) * [Math]::PI / 180.0
    Disc ($cx + [Math]::Cos($deg) * $rr) ($cy + [Math]::Sin($deg) * $rr) ($sw / 2.0)
}
$y = [single]($cy - ($rr + $sw * 0.62))
$bot = [single]($cy + $rr * 0.10)
while ($y -le $bot) { Disc $cx $y ($sw / 2.0); $y += 0.4 }

for ($y = 0; $y -lt $Size; $y++) {
    for ($x = 0; $x -lt $Size; $x++) {
        $g = $cover[$y * $Size + $x]
        if ($g -le 0.0) { continue }
        $i = ($y * $Size + $x) * 4
        $inv = 1.0 - $g
        $px[$i]     = $px[$i]     * $inv + $GLYPH[0] * $g
        $px[$i + 1] = $px[$i + 1] * $inv + $GLYPH[1] * $g
        $px[$i + 2] = $px[$i + 2] * $inv + $GLYPH[2] * $g
    }
}

# --- pull the matching uncompressed DIB frame out of the .ico ----------------
# The bytes are parsed directly rather than through System.Drawing.Icon: that
# class hands back a bitmap whose alpha has been through GDI+ conversions, and
# comparing against it reports differences that are not in the file.
$raw   = [System.IO.File]::ReadAllBytes($IcoPath)
$count = [BitConverter]::ToUInt16($raw, 4)
$frame = $null
for ($i = 0; $i -lt $count; $i++) {
    $o  = 6 + 16 * $i
    $fw = if ($raw[$o] -eq 0) { 256 } else { [int]$raw[$o] }
    if ($fw -ne $Size) { continue }
    $len = [BitConverter]::ToUInt32($raw, $o + 8)
    $off = [BitConverter]::ToUInt32($raw, $o + 12)
    if ($raw[$off] -eq 0x89) { throw "the $Size px frame is PNG-compressed and cannot be compared" }
    $frame = $raw
    $frameOff = $off
    break
}
if ($null -eq $frame) { throw "app.ico has no uncompressed $Size px frame - re-run assets/New-Icon.ps1" }

$cell = [int]($Size / $Blocks)
$nb   = $Blocks * $Blocks
$sumA = [double[]]::new($nb * 4)
$sumB = [double[]]::new($nb * 4)
$hits = [int[]]::new($nb)

# NB: [int] ROUNDS in PowerShell - [int](63/8) is 8, not 7 - which silently
# makes the grid 9x9 and overruns the arrays. Floor it, the way Rust's integer
# division already does.
for ($ry = 0; $ry -lt $Size; $ry++) {
    # ICO XOR bitmaps are stored bottom-up, in BGRA.
    $src = ($Size - 1 - $ry) * $Size * 4 + 40 + $frameOff
    for ($rx = 0; $rx -lt $Size; $rx++) {
        $blk  = [int][Math]::Floor($ry / $cell) * $Blocks + [int][Math]::Floor($rx / $cell)
        $base = $blk * 4
        $s    = $src + $rx * 4
        $d    = ($ry * $Size + $rx) * 4
        $aA   = $frame[$s + 3] / 255.0
        $aB   = $px[$d + 3] / 255.0
        # premultiplied: the .ico stores RGB 0 where alpha is 0, while the Rust
        # buffer keeps the plate colour, and an unpremultiplied compare reads
        # the squircle outline as a big difference.
        $sumA[$base + 0] += $frame[$s + 2] * $aA;  $sumB[$base + 0] += $px[$d + 0] * $aB
        $sumA[$base + 1] += $frame[$s + 1] * $aA;  $sumB[$base + 1] += $px[$d + 1] * $aB
        $sumA[$base + 2] += $frame[$s]     * $aA;  $sumB[$base + 2] += $px[$d + 2] * $aB
        $sumA[$base + 3] += $frame[$s + 3];         $sumB[$base + 3] += $px[$d + 3]
        $hits[$blk]++
    }
}

$worst = 0.0; $worstAt = ''
for ($blk = 0; $blk -lt $nb; $blk++) {
    if ($hits[$blk] -eq 0) { continue }
    for ($c = 0; $c -lt 4; $c++) {
        $a = $sumA[$blk * 4 + $c] / $hits[$blk]
        $b = $sumB[$blk * 4 + $c] / $hits[$blk]
        $d = [Math]::Abs($a - $b)
        if ($d -gt $worst) {
            $worst = $d
            $worstAt = "block $blk (col $(($blk % $Blocks) * $cell), row $([int]($blk / $Blocks) * $cell)) ch$c ico=$([Math]::Round($a,1)) rust=$([Math]::Round($b,1))"
        }
    }
}
$icoBmp = $null

Write-Host ("  worst block-average delta : {0:N1}   at {1}" -f $worst, $worstAt)
Write-Host ("  tolerance                 : {0}   ({1}x{1} blocks over {2}px)" -f $Tolerance, $Blocks, $Size)
if ($worst -gt $Tolerance) {
    Write-Host '  MISMATCH - src/icon.rs has drifted from assets/app.ico'
    exit 1
}
Write-Host '  OK - src/icon.rs and assets/app.ico agree'