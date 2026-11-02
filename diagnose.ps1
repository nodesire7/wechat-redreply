$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'WechatRedReply.exe'
$report = Join-Path $PSScriptRoot 'wechat-diagnostics.json'
$arguments = @('--diagnose','--report',('"' + $report + '"'))
$process = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0) { throw "Diagnostic command failed: $($process.ExitCode)" }
$result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
Write-Host "Report: $report"
Write-Host "Windows build: $($result.windows.build)  Server: $($result.windows.server)"
Write-Host "Binding valid: $($result.checks.binding.valid)"
Write-Host "UI nodes: $($result.checks.accessibility.nodes)  Chat input: $($result.checks.accessibility.has_chat_input)  Search: $($result.checks.accessibility.has_search_control)"
Write-Host 'Read-only. No chat contents, contact names or tokens are collected.'
