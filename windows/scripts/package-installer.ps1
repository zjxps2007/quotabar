param([string]$PackagePath, [string]$ExpectedPackageSha256)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$version = (Get-Content -LiteralPath (Join-Path $repo 'shared\version.json') -Raw | ConvertFrom-Json).version
if (-not $PackagePath) { $PackagePath = Join-Path $repo "artifacts\QuotaBar-$version-Windows-x64.zip" }
$PackagePath = (Resolve-Path -LiteralPath $PackagePath).Path
$packageHash = (Get-FileHash -LiteralPath $PackagePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($ExpectedPackageSha256 -and $packageHash -ne $ExpectedPackageSha256.ToLowerInvariant()) { throw 'Application ZIP does not match the expected release.' }
$compiler = & "$PSScriptRoot\get-inno.ps1"
$stage = Join-Path $repo ('artifacts\windows\installer-' + [Guid]::NewGuid().ToString('N'))
[IO.Compression.ZipFile]::ExtractToDirectory($PackagePath, $stage)
$payload = Join-Path $stage 'QuotaBar-Windows-x64'
foreach ($relative in @('QuotaBar.Monitor.exe','Dashboard\QuotaBar.Dashboard.exe','StatusLine\QuotaBar.StatusLine.exe','LICENSE','NOTICE')) {
    if (-not (Test-Path -LiteralPath (Join-Path $payload $relative))) { throw "Package file missing: $relative" }
}
$productVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo((Join-Path $payload 'QuotaBar.Monitor.dll')).ProductVersion
if ($productVersion -notmatch ('^' + [Regex]::Escape($version) + '\+([a-f0-9]{40})$')) { throw 'Package version or source revision mismatch.' }
$applicationCommit = $matches[1]
foreach ($relative in @('Dashboard\QuotaBar.Dashboard.dll','StatusLine\QuotaBar.StatusLine.dll')) {
    if ([Diagnostics.FileVersionInfo]::GetVersionInfo((Join-Path $payload $relative)).ProductVersion -ne $productVersion) { throw 'Mixed application revisions in package.' }
}
$output = Join-Path $repo 'artifacts'
& $compiler '--quiet' '--no-ide-signtools' ('--define=AppVersion=' + $version) ('--define=PayloadDir=' + $payload) ('--define=OutputPath=' + $output) (Join-Path $repo 'windows\installer\QuotaBar.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
$installer = Join-Path $output "QuotaBar-$version-Windows-x64-Setup.exe"
$metadata = [ordered]@{
    schema_version = 1
    version = $version
    application_commit = $applicationCommit
    installer_commit = (git -C $repo rev-parse HEAD)
    installer_source_dirty = [bool](git -C $repo status --porcelain)
    compiler = 'Inno Setup 7.1.0 x64'
    authenticode_signed = $false
    application_zip = [ordered]@{ name = [IO.Path]::GetFileName($PackagePath); sha256 = $packageHash }
    installer = [ordered]@{ name = [IO.Path]::GetFileName($installer); bytes = (Get-Item -LiteralPath $installer).Length; sha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() }
}
$metadata | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $repo 'artifacts\windows\installer-build-info.json') -Encoding utf8
[IO.File]::WriteAllText((Join-Path $repo 'artifacts\windows\installer-payload-path.txt'), $payload)
Write-Output "Installer: $installer"
Write-Output "SHA256: $($metadata.installer.sha256)"
