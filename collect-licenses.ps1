$ErrorActionPreference='Stop'
Push-Location $PSScriptRoot
try {
    $metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed' }
    $builder=[System.Text.StringBuilder]::new()
    [void]$builder.AppendLine('Wechat RedReply Rust — third-party notices')
    [void]$builder.AppendLine('SQLite is in the public domain. Windows system libraries are provided by Windows.')
    foreach ($package in ($metadata.packages | Sort-Object name,version)) {
        [void]$builder.AppendLine("`n===== $($package.name) $($package.version) | $($package.license) =====")
        $directory=Split-Path -Parent $package.manifest_path
        $files=@(Get-ChildItem -LiteralPath $directory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' })
        if ($package.license_file) {
            $extra=Join-Path $directory $package.license_file
            if (Test-Path -LiteralPath $extra) { $files+=Get-Item -LiteralPath $extra }
        }
        foreach ($file in ($files | Sort-Object FullName -Unique)) {
            [void]$builder.AppendLine($file.Name)
            [void]$builder.AppendLine([System.IO.File]::ReadAllText($file.FullName))
        }
    }
    foreach ($file in (Get-ChildItem -LiteralPath licenses -File)) {
        [void]$builder.AppendLine("`n===== $($file.Name) =====")
        [void]$builder.AppendLine([System.IO.File]::ReadAllText($file.FullName))
    }
    $destination=Join-Path $PSScriptRoot 'assets/THIRD_PARTY_LICENSES.txt'
    $content=$builder.ToString()
    if (-not (Test-Path -LiteralPath $destination) -or [System.IO.File]::ReadAllText($destination) -cne $content) {
        [System.IO.File]::WriteAllText($destination,$content,[System.Text.UTF8Encoding]::new($false))
    }
} finally { Pop-Location }
