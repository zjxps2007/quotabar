param([switch]$Demo, [switch]$Background)
$ErrorActionPreference = 'Stop'
$RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$app = Join-Path $RepoRoot 'artifacts\windows\native-Release'
$exe = Join-Path $app 'QuotaBar.Monitor.exe'
if (-not (Test-Path -LiteralPath $exe)) { & "$PSScriptRoot\build-native.ps1" -Destination (Join-Path $app 'StatusLine') }
$parameters = @{ FilePath = $exe; WindowStyle = 'Hidden'; PassThru = $true }
if ($Demo) { $parameters.ArgumentList = @('--demo') }
elseif ($Background) { $parameters.ArgumentList = @('--background') }
Start-Process @parameters | Select-Object Id, ProcessName
