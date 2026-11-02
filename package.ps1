param([switch]$SkipBuild, [string]$OutputDirectory = 'release')
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    if (-not $SkipBuild) {
        & "$PSScriptRoot\build.ps1"
        if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
    }
    $versionLine = Get-Content Cargo.toml | Where-Object { $_ -match '^version\s*=\s*"([^"]+)"' } | Select-Object -First 1
    $version = [regex]::Match($versionLine, '"([^"]+)"').Groups[1].Value
    if (-not $version) { throw 'Missing version' }
    $output = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot $OutputDirectory))
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    $name = "WechatRedReply-v$version-windows-x64"
    $stage = Join-Path $output $name
    if (Test-Path -LiteralPath $stage) { throw "Output already exists: $stage" }
    New-Item -ItemType Directory -Path $stage | Out-Null
    foreach ($file in @('WechatRedReply.exe','THIRD_PARTY_LICENSES.txt')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot "dist-build/$file") -Destination (Join-Path $stage $file)
    }
    foreach ($file in @('README.md','QUICKSTART.md','LICENSE','NOTICE','start.cmd','install.cmd','install.ps1','diagnose.cmd','diagnose.ps1')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination (Join-Path $stage $file)
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'redlang') -Destination $stage -Recurse
    $manifest = [ordered]@{
        version = $version
        platform = 'windows-x64'
        verified_wechat = '4.2.0.9'
        additional_ui_verification = @{ windows='Server 2019 build 17763'; wechat='4.2.0.12'; operations=@('recover chat page','group text','split text and image','recall test receipt') }
        native_music_verified_wechat = '4.2.0.9'
        configuration_included = $false
        runtime_data_included = $false
        runtime_dependencies = @('Windows x64','Logged-in WeChat','Existing RedReply')
        files = @(Get-ChildItem -LiteralPath $stage -File -Recurse | Sort-Object FullName | ForEach-Object {
            [ordered]@{path=$_.FullName.Substring($stage.Length+1).Replace('\','/');size=$_.Length;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
        })
    }
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText((Join-Path $stage 'release-manifest.json'), ($manifest | ConvertTo-Json -Depth 6), $utf8)
    $zip = Join-Path $output "$name.zip"
    Compress-Archive -LiteralPath $stage -DestinationPath $zip -CompressionLevel Optimal
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    $standalone = Join-Path $output 'WechatRedReply.exe'
    Copy-Item -LiteralPath (Join-Path $stage 'WechatRedReply.exe') -Destination $standalone
    $exeHash = (Get-FileHash -LiteralPath $standalone -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText((Join-Path $output 'SHA256SUMS.txt'), "$hash  $name.zip`n$exeHash  WechatRedReply.exe`n", $utf8)
    Write-Host "Package: $zip"
    Write-Host "SHA256: $hash"
} finally { Pop-Location }

