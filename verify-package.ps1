param([Parameter(Mandatory=$true)][string]$Archive)
$ErrorActionPreference = 'Stop'
$archivePath = [System.IO.Path]::GetFullPath($Archive)
$sandbox = Join-Path ([System.IO.Path]::GetTempPath()) ('WechatRedReply-package-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sandbox | Out-Null
Expand-Archive -LiteralPath $archivePath -DestinationPath $sandbox
$extracted = @(Get-ChildItem -LiteralPath $sandbox -Directory)
if ($extracted.Count -ne 1) { throw 'Expected one root directory in archive' }
$app = $extracted[0].FullName
if ((Test-Path -LiteralPath (Join-Path $app 'config.json')) -or (Test-Path -LiteralPath (Join-Path $app 'data'))) { throw 'Archive contains user state' }
$manifest = Get-Content -LiteralPath (Join-Path $app 'release-manifest.json') -Raw | ConvertFrom-Json
foreach ($file in $manifest.files) {
    $item = Join-Path $app $file.path
    if ((Get-FileHash -LiteralPath $item -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) { throw "File hash mismatch: $($file.path)" }
}
$exe = Join-Path $app 'WechatRedReply.exe'
& $exe --validate
if ($LASTEXITCODE -ne 0) { throw 'First-run configuration failed' }
$first = Get-Content -LiteralPath (Join-Path $app 'config.json') -Raw | ConvertFrom-Json
if ($first.redreply.access_token.Length -lt 24 -or $first.whitelist.Count -ne 0 -or $first.group_whitelist.Count -ne 0 -or $first.wechat_binding) { throw 'Unexpected initial account state' }
$moved = Join-Path $sandbox ('Moved with spaces ' + [char]0x8FC1 + [char]0x79FB)
foreach ($movePath in @($app,$moved)) {
    if (-not [System.IO.Path]::GetFullPath($movePath).StartsWith(($sandbox + [System.IO.Path]::DirectorySeparatorChar), [System.StringComparison]::OrdinalIgnoreCase)) { throw 'Move path escaped verification directory' }
}
Move-Item -LiteralPath $app -Destination $moved
& (Join-Path $moved 'WechatRedReply.exe') --validate
if ($LASTEXITCODE -ne 0) { throw 'Relocation failed' }
$second = Get-Content -LiteralPath (Join-Path $moved 'config.json') -Raw | ConvertFrom-Json
if ($second.redreply.access_token -ne $first.redreply.access_token) { throw 'Relocation changed token' }
$installed = Join-Path $sandbox 'Installed with spaces'
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $moved 'install.ps1') -Destination $installed -NoLaunch -NoShortcut
if ($LASTEXITCODE -ne 0) { throw 'Installer failed' }
& (Join-Path $installed 'WechatRedReply.exe') --validate
if ($LASTEXITCODE -ne 0) { throw 'Installed executable failed' }
$installedConfig = Join-Path $installed 'config.json'
$beforeHash = (Get-FileHash -LiteralPath $installedConfig -Algorithm SHA256).Hash
$installedState = Get-Content -LiteralPath $installedConfig -Raw | ConvertFrom-Json
if ($installedState.redreply.access_token -eq $first.redreply.access_token) { throw 'Fresh installs share a token' }
$dummyData = Join-Path $installed 'data'
New-Item -ItemType Directory -Path $dummyData | Out-Null
[System.IO.File]::WriteAllText((Join-Path $dummyData 'preserve.txt'), 'existing-data')
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $moved 'install.ps1') -Destination $installed -NoLaunch -NoShortcut
if ($LASTEXITCODE -ne 0) { throw 'Upgrade failed' }
if ((Get-FileHash -LiteralPath $installedConfig -Algorithm SHA256).Hash -ne $beforeHash) { throw 'Upgrade replaced configuration' }
if ([System.IO.File]::ReadAllText((Join-Path $dummyData 'preserve.txt')) -ne 'existing-data') { throw 'Upgrade replaced data' }
& (Join-Path $installed 'WechatRedReply.exe') --licenses
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath (Join-Path $installed 'THIRD_PARTY_LICENSES.txt'))) { throw 'Embedded licenses unavailable' }
Write-Host 'PASS: archive hashes, clean state, first run, independent tokens, relocation, install, upgrade preserving data, embedded licenses.'
Write-Host "Verification files: $sandbox"

