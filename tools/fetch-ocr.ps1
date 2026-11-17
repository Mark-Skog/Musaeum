# Downloads the pinned sources of Tesseract and Leptonica, and the recognition models, into
# vendor/tesseract/. Safe to run repeatedly.
#
# crates/musaeum-tesseract compiles the two libraries with the `cc` crate and links them statically
# into the OCR worker, as crates/musaeum-djvu does with DjVuLibre. They are not kept in the
# repository because together they are some 17 MB of C and C++; every digest below was recorded on
# the first download, so any later change to an archive stops the script instead of silently
# swapping the engine. Leptonica's matches the digest GitHub publishes for its release asset.
$ErrorActionPreference = 'Stop'

$target = Join-Path $PSScriptRoot '..\vendor\tesseract'
New-Item -ItemType Directory -Force -Path $target | Out-Null
# Named explicitly because a Git Bash PATH puts GNU tar first, which reads "C:" as a host.
$tar = "$env:SystemRoot\System32\tar.exe"

function Get-Pinned([string] $url, [string] $file, [string] $sha256) {
    if (-not (Test-Path $file)) {
        Write-Host "Downloading $url"
        & curl.exe -fsSL -o $file $url
        if ($LASTEXITCODE -ne 0) { throw "curl exited with $LASTEXITCODE" }
    }
    $actual = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
    if ($actual -ne $sha256) {
        Remove-Item $file
        throw "SHA-256 mismatch for ${file}: expected $sha256, got $actual"
    }
}

$sources = @(
    @{
        Name = 'tesseract-5.5.4'
        Url = 'https://github.com/tesseract-ocr/tesseract/archive/refs/tags/5.5.4.tar.gz'
        Sha256 = 'a8dbf8b8c8ee427ff6ad1d99116502feaf59440af1adb893caf60a6576826cf4'
    },
    @{
        Name = 'leptonica-1.87.0'
        Url = 'https://github.com/DanBloomberg/leptonica/releases/download/1.87.0/leptonica-1.87.0.tar.gz'
        Sha256 = 'c73363397f96eb1295602bf44d708a994ad42046c791bf03ea0505d829bdb6a7'
    }
)
foreach ($source in $sources) {
    $archive = Join-Path $target "$($source.Name).tar.gz"
    Get-Pinned $source.Url $archive $source.Sha256
    $folder = Join-Path $target $source.Name
    if (-not (Test-Path $folder)) {
        & $tar -xzf $archive -C $target
        if ($LASTEXITCODE -ne 0) { throw "tar exited with $LASTEXITCODE" }
    }
}

# The "fast" LSTM models of tessdata_fast 4.1.0 (Apache-2.0): integer weights, a few megabytes
# each, which is what a selection waits on.
$models = Join-Path $target 'tessdata'
New-Item -ItemType Directory -Force -Path $models | Out-Null
$base = 'https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/4.1.0'
Get-Pinned "$base/por.traineddata" (Join-Path $models 'por.traineddata') 'c4932b937207a9514b7514d518b931a99938c02a28a5a5a553f8599ed58b7deb'
Get-Pinned "$base/eng.traineddata" (Join-Path $models 'eng.traineddata') '7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2'
Get-Pinned "$base/LICENSE" (Join-Path $models 'LICENSE') 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'

Write-Host "Tesseract 5.5.4, Leptonica 1.87.0 and the models are ready in $target"
