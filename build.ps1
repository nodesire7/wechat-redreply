param([switch]$SkipTests,[string]$OutputDirectory='dist-build')
$ErrorActionPreference='Stop'
Push-Location $PSScriptRoot
try {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Install Rust MSVC and Visual Studio C++ Build Tools before building.' }
    if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) { throw 'Install Node.js and pnpm for frontend builds; the distributed EXE does not need them.' }
    if (-not (Test-Path -LiteralPath 'design/gui-react/node_modules')) {
        pnpm --dir design/gui-react install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency installation failed' }
    }
    pnpm --dir design/gui-react test
    if ($LASTEXITCODE -ne 0) { throw 'Frontend behavior tests failed' }
    pnpm --dir design/gui-react build
    if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
    if (-not $SkipTests) {
        cargo test --locked
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
    }
    cargo build --locked --release -p wechat-offscreen-guard
    if ($LASTEXITCODE -ne 0) { throw 'Offscreen helper build failed' }
    Copy-Item -LiteralPath target/release/wechat_offscreen_guard.dll -Destination assets/offscreen-guard.dll -Force
    & "$PSScriptRoot\collect-licenses.ps1"
    cargo build --locked --release -p wechat-redreply
    if ($LASTEXITCODE -ne 0) { throw 'EXE build failed' }
    $output=if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {$OutputDirectory} else {Join-Path $PSScriptRoot $OutputDirectory}
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    Copy-Item -LiteralPath target/release/wechat-redreply.exe -Destination (Join-Path $output 'WechatRedReply.exe') -Force
    Copy-Item -LiteralPath README.md -Destination (Join-Path $output 'README.md') -Force
    Copy-Item -LiteralPath assets/THIRD_PARTY_LICENSES.txt -Destination (Join-Path $output 'THIRD_PARTY_LICENSES.txt') -Force
    Get-FileHash -LiteralPath (Join-Path $output 'WechatRedReply.exe') -Algorithm SHA256 | Format-List
} finally { Pop-Location }
