param(
    [string]$Destination = (Join-Path $env:LOCALAPPDATA 'WechatRedReply'),
    [switch]$NoShortcut,
    [switch]$NoLaunch
)
$ErrorActionPreference = 'Stop'
$sourceExe = Join-Path $PSScriptRoot 'WechatRedReply.exe'
if (-not (Test-Path -LiteralPath $sourceExe)) { throw 'Extract the complete release ZIP before installing.' }
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
New-Item -ItemType Directory -Force -Path $destinationPath | Out-Null
$targetExe = Join-Path $destinationPath 'WechatRedReply.exe'
if ($sourceExe -ne $targetExe) {
    # Existing config.json and data are deliberately absent from this file list.
    foreach ($name in @('WechatRedReply.exe','README.md','QUICKSTART.md','LICENSE','NOTICE','THIRD_PARTY_LICENSES.txt','start.cmd','install.cmd','install.ps1','diagnose.cmd','diagnose.ps1','release-manifest.json')) {
        $source = Join-Path $PSScriptRoot $name
        if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination (Join-Path $destinationPath $name) -Force }
    }
    if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'redlang')) {
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'redlang') -Destination $destinationPath -Recurse -Force
    }
}
if (-not $NoShortcut) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Programs')) 'Wechat RedReply.lnk'))
    $shortcut.TargetPath = $targetExe
    $shortcut.WorkingDirectory = $destinationPath
    $shortcut.Save()
}
Write-Host "Installed: $targetExe"
if (-not $NoLaunch) { Start-Process -FilePath $targetExe -WorkingDirectory $destinationPath }

