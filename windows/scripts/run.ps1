param([switch]$Demo, [switch]$Background)
. "$PSScriptRoot\common.ps1"
$exe = Join-Path $RepoRoot 'windows\src\QuotaBar.Monitor\bin\Release\net10.0-windows\QuotaBar.Monitor.exe'
if (-not (Test-Path -LiteralPath $exe)) { & "$PSScriptRoot\build.ps1" }
$parameters = @{ FilePath = $exe; WindowStyle = 'Hidden'; PassThru = $true }
if ($Demo) { $parameters.ArgumentList = @('--demo') }
elseif ($Background) { $parameters.ArgumentList = @('--background') }
Start-Process @parameters | Select-Object Id, ProcessName
