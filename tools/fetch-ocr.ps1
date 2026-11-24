# Downloads the pinned sources of Tesseract and Leptonica, and the recognition models, into
# vendor/tesseract/. Safe to run repeatedly.
#
# crates/musaeum-tesseract compiles the two libraries with the `cc` crate and links them statically
# into the OCR worker, as crates/musaeum-djvu does with DjVuLibre. They are not kept in the
# repository because together they are some 17 MB of C and C++; every digest below was recorded on
# the first download, so any later change to an archive stops the script instead of silently
# swapping the engine. Leptonica's matches the digest GitHub publishes for its release asset.
#
# -Languages adds models besides Portuguese and English, among those pinned below (the same
# SHA-256 crates/musaeum-ocr/src/models.rs holds, so the app trusts them too):
#   powershell -ExecutionPolicy Bypass -File tools\fetch-ocr.ps1 -Languages spa,fra
# then build the app again (`npm run build:app`): the build copies every model next to the worker,
# and the app's Preferences list them. A model put there by hand, or changed, is never used.
param([string[]] $Languages = @())
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

# The others the app trusts, fetched only when asked for.
$optional = @{
    spa = '6f2e04d02774a18f01bed44b1111f2cd7f3ba7ac9dc4373cd3f898a40ea6b464'
    fra = 'ced037562e8c80c13122dece28dd477d399af80911a28791a66a63ac1e3445ca'
    deu = '19d219bbb6672c869d20a9636c6816a81eb9a71796cb93ebe0cb1530e2cdb22d'
    ita = 'b8f89e1e785118dac4d51ae042c029a64edb5c3ee42ef73027a6d412748d8827'
    lat = '3859d8ba60404f4b79830622625bbc76fb4ee2808eac1ad360ffa77f0a533328'
}
foreach ($language in ($Languages -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })) {
    if (-not $optional.ContainsKey($language)) {
        throw "$language is not a model this script knows: $($optional.Keys -join ', ')"
    }
    Get-Pinned "$base/$language.traineddata" (Join-Path $models "$language.traineddata") $optional[$language]
}

Write-Host "Tesseract 5.5.4, Leptonica 1.87.0 and the models are ready in $target"
