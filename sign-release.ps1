# =============================================================================
#  sign-release.ps1 - Authenticode-sign the built executable.
# =============================================================================
#  Works with any of the certificate routes described in SIGNING.md, because
#  they all end in a `signtool sign` command line:
#
#    * a .pfx on disk
#    * a certificate in the Windows store
#    * Azure Artifact Signing (digest signing against Microsoft's endpoint,
#      where the private key never leaves their HSM)
#
#  GitHub Actions calls this automatically when the signing secrets exist, and
#  skips it silently when they do not, so an unsigned build still works for
#  local testing.
#
#  Usage:
#    pwsh -File sign-release.ps1 -ExePath .\XMG-Optimizer.exe
# =============================================================================

param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,

    # Route A: a .pfx plus its password.
    [string]$PfxPath   = $env:XMG_SIGN_PFX_PATH,
    [string]$PfxPassword = $env:XMG_SIGN_PFX_PASSWORD,

    # Route B: Azure Artifact Signing. Leave both blank to skip signing.
    [string]$AzureEndpoint   = $env:XMG_SIGN_AZURE_ENDPOINT,
    [string]$AzureCertProfile = $env:XMG_SIGN_AZURE_CERT_PROFILE,

    # Countersignature authority. This is not optional in practice: without a
    # timestamp the signature stops validating the moment the certificate
    # expires, and code-signing certificates are short-lived.
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'

$exe = (Resolve-Path $ExePath).Path

# --- locate signtool ---------------------------------------------------------
# It ships with the Windows SDK, which is present on GitHub's windows runners
# but not necessarily on a developer machine.
function Find-SignTool {
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $roots = @(
        "${env:ProgramFiles(x86)}\Windows Kits\10\bin",
        "$env:ProgramFiles\Windows Kits\10\bin"
    )
    foreach ($r in $roots) {
        if (-not (Test-Path $r)) { continue }
        # Prefer the x64 build, then fall back to whatever exists.
        $hit = Get-ChildItem $r -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
               Sort-Object FullName -Descending |
               Where-Object { $_.FullName -match 'x64' } |
               Select-Object -First 1
        if ($hit) { return $hit.FullName }
        $any = Get-ChildItem $r -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($any) { return $any.FullName }
    }
    return $null
}

$signtool = Find-SignTool
if (-not $signtool) {
    throw "signtool.exe not found. Install the Windows SDK, or skip signing (an unsigned build still runs after Unblock-File)."
}
Write-Host "  signtool : $signtool"

# --- build the argument list -------------------------------------------------
# /fd and /td must both be SHA256. /a selects the best matching certificate
# automatically when signing from the store.
$args = @(
    'sign',
    '/fd', 'SHA256',
    '/td', 'SHA256',
    '/tr', $TimestampUrl
)

$route = 'none'

if ($AzureEndpoint -and $AzureCertProfile) {
    # Route B: digest signing. signtool uploads only a SHA256 digest; the
    # private key stays in Microsoft's HSM.
    $route = 'azure-artifact-signing'
    $args += @(
        '/ds', $AzureEndpoint,
        '/dc', $AzureCertProfile,
        '/dlib', 'azuresigning.dll'
    )
}
elseif ($PfxPath -and (Test-Path $PfxPath)) {
    # Route A: a pfx on disk.
    $route = 'pfx'
    $args += @('/f', (Resolve-Path $PfxPath).Path)
    if ($PfxPassword) { $args += @('/p', $PfxPassword) }
}
else {
    Write-Host "  no signing material configured - leaving the executable unsigned"
    Write-Host "  (set XMG_SIGN_PFX_PATH / XMG_SIGN_PFX_PASSWORD, or the Azure variables)"
    exit 0
}

$args += $exe

Write-Host "  route    : $route"
Write-Host "  signing  : $(Split-Path -Leaf $exe)"

# Never echo the password into the log.
& $signtool @args
if ($LASTEXITCODE -ne 0) {
    throw "signtool failed with exit code $LASTEXITCODE"
}

# --- verify ------------------------------------------------------------------
# Never trust that signing succeeded because the command exited zero. Read the
# signature back out of the file.
$verifyArgs = @('verify', '/pa', '/v', $exe)
& $signtool @verifyArgs | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "the signature did not verify after signing (exit $LASTEXITCODE) - not shipping this"
}

$sig = Get-AuthenticodeSignature $exe
Write-Host ""
Write-Host ("  signature status : {0}" -f $sig.Status)
Write-Host ("  subject          : {0}" -f $sig.SignerCertificate.Subject)
Write-Host ("  thumbprint       : {0}" -f $sig.SignerCertificate.Thumbprint)
Write-Host ""
Write-Host "  [ok] signed and verified"