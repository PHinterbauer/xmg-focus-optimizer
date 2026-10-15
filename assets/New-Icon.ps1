<#
================================================================================
 XMG Focus Optimizer - icon generator
================================================================================
 Draws at 1024 px with true vector geometry, then downsamples to every size.

 Why draw big and shrink: antialiasing done once at high resolution and
 filtered down is what separates a drawn icon from a procedural one. Stamping
 hundreds of little discs leaves a bumpy edge; a real stroked path does not.

 The plate is a genuine superellipse (n = 4, the "continuous corner" shape),
 sampled from its parametric form rather than approximated with straight bezier
 segments, so curvature stays smooth right into the corner.

 The design is deliberately flat: one solid fill and one solid glyph. No
 gradient, no sheen, no rim light, no drop shadow. Flat survives being scaled
 to 16 px in the notification area far better than anything with depth in it.

 Output is a real multi-resolution .ico:
   * below 256 px  -> uncompressed DIB, because GDI+ cannot read PNG frames
   * 256 px        -> PNG, which is what Explorer expects at the large size

 Re-run after editing, then commit the regenerated assets:
   pwsh -File assets/New-Icon.ps1
================================================================================
#>
param(
    [string]$OutputPath  = (Join-Path $PSScriptRoot 'app.ico'),
    [string]$PreviewPath = (Join-Path $PSScriptRoot 'icon-preview.png'),
    [string]$PngPath     = (Join-Path $PSScriptRoot 'icon.png')
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$sizes = @(16, 24, 32, 48, 64, 128, 256)
$SS    = 1024                       # supersampling canvas

# ------------------------------------------------------------------ palette --
#
# One colour. Deep violet, saturated enough to stay legible against a dark
# taskbar and bright enough to read against a light one. The white glyph on top
# of it is roughly 5.9:1 contrast, which is why the glyph is white rather than
# another tint.
$PLATE = [System.Drawing.Color]::FromArgb(255, 109, 40, 217)   # #6D28D9
$GLYPH = [System.Drawing.Color]::White

# ------------------------------------------------------------------ shape --

# Appends a superellipse |x/a|^n + |y/b|^n = 1 to an existing path.
# Filling a supplied path rather than returning one avoids PowerShell
# unrolling GraphicsPath (it is IEnumerable) into a bag of PointF.
function Add-Superellipse($Path, [single]$cx, [single]$cy, [single]$a, [single]$b, [double]$n = 4.0) {
    $steps = 720
    $pts = New-Object 'System.Drawing.PointF[]' $steps
    $e = 2.0 / $n
    for ($i = 0; $i -lt $steps; $i++) {
        $t = 2.0 * [Math]::PI * $i / $steps
        $c = [Math]::Cos($t); $s = [Math]::Sin($t)
        $x = $cx + $a * [Math]::Sign($c) * [Math]::Pow([Math]::Abs($c), $e)
        $y = $cy + $b * [Math]::Sign($s) * [Math]::Pow([Math]::Abs($s), $e)
        $pts[$i] = New-Object System.Drawing.PointF -ArgumentList ([single]$x), ([single]$y)
    }
    $Path.AddPolygon($pts)
    $Path.CloseFigure()
}

# ---------------------------------------------------------------- bitmap ---

function New-AppBitmap([int]$Size) {
    $small = New-Object System.Drawing.Bitmap -ArgumentList $Size, $Size
    $small.SetResolution(96, 96)

    $big = New-Object System.Drawing.Bitmap -ArgumentList $SS, $SS
    $big.SetResolution(96, 96)
    $g = [System.Drawing.Graphics]::FromImage($big)
    $g.SmoothingMode      = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode  = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode    = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    # ---- geometry -----------------------------------------------------
    $pad  = [single]($SS * 0.078)
    $side = [single]($SS - 2 * $pad)
    $c    = [single]($SS / 2.0)
    $a    = [single]($side / 2.0)

    # NB: named $shape, not $plate - PowerShell variables are
    # case-insensitive, so a local $plate here would clobber the $PLATE colour
    # constant above.
    $shape = New-Object System.Drawing.Drawing2D.GraphicsPath
    Add-Superellipse $shape $c $c $a $a 4.0

    # ---- plate: one flat fill ----------------------------------------
    $solid = New-Object System.Drawing.SolidBrush -ArgumentList $PLATE
    $g.FillPath($solid, $shape)

    # ---- glyph: a power symbol, drawn as real strokes ------------------
    #
    # GDI+ measures angles clockwise from 3 o'clock, so 270 deg is straight up.
    # A 270 deg sweep from -45 deg leaves a 90 deg gap centred on 270 deg, which
    # is where the stem sits. The previous drawing put the gap on the left while
    # the stem was vertical at the top, which drew a broken symbol at runtime.
    $gR   = [single]($side * 0.205)
    $sw   = [single]($side * 0.090)
    $pen  = New-Object System.Drawing.Pen -ArgumentList $GLYPH, $sw
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round

    # DrawArc strokes the ellipse DEFINED by the rect; the pen straddles it.
    # So the rect must be exactly the centreline, 2*gR across, anchored on the
    # centre. Widening it by the pen width puts the centreline at radius
    # gR + sw/2 and moves its centre to c + sw/2, which slides the ring down and
    # right until it no longer lines up with the stem drawn at x = c.
    $dia  = [single](2 * $gR)
    $rect = [System.Drawing.RectangleF]::new([single]($c - $gR), [single]($c - $gR), $dia, $dia)
    $g.DrawArc($pen, $rect, -45.0, 270.0)

    # Stem rises through the gap to just past the ring's outer edge and stops a
    # little below the centre, so the symbol keeps its balance at small sizes.
    $stemTop = [single]($c - ($gR + $sw * 0.62))
    $stemBot = [single]($c + $gR * 0.10)
    $g.DrawLine($pen, $c, $stemTop, $c, $stemBot)
    $pen.Dispose()

    $shape.Dispose(); $solid.Dispose()
    $g.Dispose()

    # ---- downsample ----------------------------------------------------
    $g2 = [System.Drawing.Graphics]::FromImage($small)
    $g2.CompositingMode    = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g2.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g2.InterpolationMode  = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g2.PixelOffsetMode    = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g2.SmoothingMode      = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g2.DrawImage($big, 0, 0, [int]$Size, [int]$Size)
    $g2.Dispose()
    $big.Dispose()

    return $small
}

# ------------------------------------------------------------- ico writer --

function ConvertTo-IcoFrame($Bitmap, [bool]$AsPng) {
    if ($AsPng) {
        $png = New-Object System.IO.MemoryStream
        $Bitmap.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
        $b = $png.ToArray(); $png.Dispose()
        return ,$b
    }

    $w = $Bitmap.Width; $h = $Bitmap.Height
    $stride = $w * 4
    $xor = New-Object byte[] ($stride * $h)

    # Read row by row and write bottom-up. LockBits returns the backing store
    # in whatever order GDI+ happened to keep it, which previously flipped the
    # icon; going through GetPixel is slower but unambiguous.
    for ($y = 0; $y -lt $h; $y++) {
        $sy = $h - 1 - $y
        $rowOff = $y * $stride
        for ($x = 0; $x -lt $w; $x++) {
            $col = $Bitmap.GetPixel($x, $sy)
            $i = $rowOff + $x * 4
            $xor[$i]     = $col.B
            $xor[$i + 1] = $col.G
            $xor[$i + 2] = $col.R
            $xor[$i + 3] = $col.A
        }
    }

    $maskStride = [int]([Math]::Floor(($w + 31) / 32) * 4)
    $mask = New-Object byte[] ($maskStride * $h)

    $out = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter -ArgumentList $out
    $bw.Write([uint32]40)                       # biSize
    $bw.Write([int32]$w)                        # biWidth
    $bw.Write([int32]($h * 2))                  # biHeight (XOR + AND)
    $bw.Write([uint16]1)                        # biPlanes
    $bw.Write([uint16]32)                       # biBitCount
    $bw.Write([uint32]0)                        # BI_RGB
    $bw.Write([uint32]($xor.Length + $mask.Length))
    $bw.Write([int32]0); $bw.Write([int32]0)
    $bw.Write([uint32]0); $bw.Write([uint32]0)
    $bw.Write($xor); $bw.Write($mask)
    $bw.Flush()
    $b = $out.ToArray(); $bw.Dispose(); $out.Dispose()
    return ,$b
}

$images = @()
foreach ($s in $sizes) { $images += (New-AppBitmap $s) }

$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter -ArgumentList $ms
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$images.Count)

$offset = 6 + (16 * $images.Count)
$blobs = @()
foreach ($bmp in $images) {
    $bytes = ConvertTo-IcoFrame $bmp ($bmp.Width -ge 256)
    $blobs += ,$bytes
    $w = if ($bmp.Width  -ge 256) { 0 } else { [int]$bmp.Width }
    $h = if ($bmp.Height -ge 256) { 0 } else { [int]$bmp.Height }
    $bw.Write([byte]$w); $bw.Write([byte]$h)
    $bw.Write([byte]0);  $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$bytes.Length)
    $bw.Write([uint32]$offset)
    $offset += $bytes.Length
}
foreach ($b in $blobs) { $bw.Write($b) }
$bw.Flush()
$data = $ms.ToArray(); $bw.Dispose(); $ms.Dispose()

$tmp = [System.IO.Path]::GetTempFileName()
[System.IO.File]::WriteAllBytes($tmp, $data)
Copy-Item $tmp $OutputPath -Force
Remove-Item $tmp -Force -ErrorAction SilentlyContinue

$preview = New-AppBitmap 256
$preview.Save($PreviewPath, [System.Drawing.Imaging.ImageFormat]::Png)
$preview.Save($PngPath,     [System.Drawing.Imaging.ImageFormat]::Png)
$preview.Dispose()
foreach ($bmp in $images) { $bmp.Dispose() }

Write-Host ("  Icon written : {0}  ({1} KB, {2} sizes)" -f $OutputPath, [math]::Round((Get-Item $OutputPath).Length / 1KB, 1), $sizes.Count)
Write-Host "  Preview      : $PreviewPath"
Write-Host "  PNG          : $PngPath"