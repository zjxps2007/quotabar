param([ValidateSet('Debug','Release')][string]$Configuration = 'Release',
      [Parameter(Mandatory=$true)][string]$Destination)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$localCargo = Join-Path $repo '.tools\rust\bin\cargo.exe'
$savedPath, $savedCargo = $env:PATH, $env:CARGO_HOME
try {
    if (Test-Path -LiteralPath $localCargo) {
        $env:PATH = (Split-Path -Parent $localCargo) + ';' + $savedPath
        $env:CARGO_HOME = Join-Path $repo '.tools\cargo'
        $cargo = $localCargo
    } else { $cargo = (Get-Command cargo -ErrorAction Stop).Source }
    $target = Join-Path $repo 'target'
    $argsForBuild = @('build','--locked','--manifest-path',(Join-Path $repo 'Cargo.toml'),'--target-dir',$target)
    if ($Configuration -eq 'Release') { $argsForBuild += '--release' }
    & $cargo @argsForBuild --package quotabar --bin quotabar-statusline
    if ($LASTEXITCODE -ne 0) { throw 'Native helper build failed.' }
    & $cargo @argsForBuild --package quotabar-native
    if ($LASTEXITCODE -ne 0) { throw 'Rust core library build failed.' }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    $program = Join-Path $target ($Configuration.ToLowerInvariant() + '\quotabar-statusline.exe')
    Copy-Item -LiteralPath $program -Destination (Join-Path $Destination 'QuotaBar.StatusLine.exe') -Force
} finally {
    $env:PATH = $savedPath
    $env:CARGO_HOME = $savedCargo
}
