param([ValidateSet('Debug','Release')][string]$Configuration = 'Release')
. "$PSScriptRoot\common.ps1"
& "$PSScriptRoot\build-native.ps1" -Configuration $Configuration -Destination (Join-Path $RepoRoot "windows\src\QuotaBar.Monitor\bin\$Configuration\net10.0-windows\StatusLine")
Push-Location (Join-Path $RepoRoot 'windows')
try { Invoke-Dotnet @('build','QuotaBar.sln','-c',$Configuration,'--nologo','-m:1','-nodeReuse:false','-p:UseSharedCompilation=false') }
finally { Pop-Location }
