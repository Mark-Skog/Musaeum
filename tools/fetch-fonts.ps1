# Downloads the pinned fonts of the study reports into vendor/fonts/. Safe to run repeatedly.
#
# crates/musaeum-report embeds them in the PDF reports it writes, as subsets: Noto Sans for the
# notes and the headings around them, Noto Serif for what the books say, Noto Sans Mono for code,
# and Noto Sans Math and Noto Sans Symbols 2 for what those three do not draw (arrows, operators,
# check marks). All are the unhinted TrueType files of the Noto project's own distribution, at one
# commit, under the SIL Open Font License 1.1; every digest below was recorded on the first
# download, so a later change to a file stops the script instead of silently changing a report.
$ErrorActionPreference = 'Stop'

$target = Join-Path $PSScriptRoot '..\vendor\fonts'
New-Item -ItemType Directory -Force -Path $target | Out-Null

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

$commit = '682229039c5a594f247890d94e3fb497be8d58dd'
$base = "https://raw.githubusercontent.com/notofonts/notofonts.github.io/$commit/fonts"
$fonts = @(
    @{ Path = 'NotoSans/unhinted/ttf/NotoSans-Regular.ttf'; Sha256 = 'f3961a9cde016d41a4879aecda1474d3a36d6bf54fa0e4643de029cc2248b0e8' },
    @{ Path = 'NotoSans/unhinted/ttf/NotoSans-Italic.ttf'; Sha256 = '678288f868807d4d64a6f3b51466871d117d915780381ce9d0ed4b3bcbd06d37' },
    @{ Path = 'NotoSans/unhinted/ttf/NotoSans-Bold.ttf'; Sha256 = '87cb2d84472a7d66da659ee47b6cdb9552326e8c128245231f191b6ac72529d9' },
    @{ Path = 'NotoSans/unhinted/ttf/NotoSans-BoldItalic.ttf'; Sha256 = '3d367743f371f28671d2764e911a53d7c20ec9b6aa8791d059e7090389fc52a5' },
    @{ Path = 'NotoSerif/unhinted/ttf/NotoSerif-Regular.ttf'; Sha256 = 'a15cfbbc1539d707115111d672d590a3d70d4f74b4c0a315956da20ae19a14e1' },
    @{ Path = 'NotoSerif/unhinted/ttf/NotoSerif-Bold.ttf'; Sha256 = '24ad531e6b05ddad8c3d89572d2c93eb86a6b74e652ce7ee3c3e171de68e84c3' },
    @{ Path = 'NotoSansMono/unhinted/ttf/NotoSansMono-Regular.ttf'; Sha256 = '87f8ce0522a6c99b743ee5fc75b4073cfdd575639119672828b7b9944b65b4f4' },
    @{ Path = 'NotoSansMath/unhinted/ttf/NotoSansMath-Regular.ttf'; Sha256 = 'b127e84699212b6b2ef50aff58e0ebebeec04ffe6db1b9eb9e209c8c3d97b4aa' },
    @{ Path = 'NotoSansSymbols2/unhinted/ttf/NotoSansSymbols2-Regular.ttf'; Sha256 = 'c4a0a80f0041ce4be81e2478faad22776d23edb98ae3f0d19bd37044820ecf9d' }
)
foreach ($font in $fonts) {
    $name = Split-Path $font.Path -Leaf
    Get-Pinned "$base/$($font.Path)" (Join-Path $target $name) $font.Sha256
}
Get-Pinned "$base/LICENSE" (Join-Path $target 'OFL.txt') 'f2095b08bed08b23a6fe26112fcd679a2bee3f002eef077eb05d215ed1051bd8'

Write-Host "The report fonts are ready in $target"
