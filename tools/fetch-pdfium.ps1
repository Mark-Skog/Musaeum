# Downloads the pinned PDFium build into vendor/. Safe to run repeatedly.
#
# The digest was recorded on the first download of this release, so any later change
# to the archive stops the script instead of silently swapping the rendering engine.
$ErrorActionPreference = 'Stop'

$release = '8125'
$archive = 'pdfium-win-x64.tgz'
$sha256 = '9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b'
$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/$release/$archive"

$target = Join-Path $PSScriptRoot "..\vendor\pdfium\chromium-$release"
New-Item -ItemType Directory -Force -Path $target | Out-Null
$file = Join-Path $target $archive

if (-not (Test-Path $file)) {
    Write-Host "Downloading $url"
    & curl.exe -fsSL -o $file $url
    if ($LASTEXITCODE -ne 0) { throw "curl exited with $LASTEXITCODE" }
}

$actual = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
if ($actual -ne $sha256) {
    Remove-Item $file
    throw "SHA-256 mismatch for ${archive}: expected $sha256, got $actual"
}

# Named explicitly because a Git Bash PATH puts GNU tar first, which reads "C:" as a host.
& "$env:SystemRoot\System32\tar.exe" -xzf $file -C $target
if ($LASTEXITCODE -ne 0) { throw "tar exited with $LASTEXITCODE" }
Write-Host "PDFium chromium/$release is ready in $target"
