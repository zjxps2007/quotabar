param([ValidateSet('win-x64')][string]$Runtime = 'win-x64',
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_-]*$')][string]$OutputFolder = 'QuotaBar-Windows-x64')
. "$PSScriptRoot\common.ps1"
$version = (Get-Content -LiteralPath (Join-Path $RepoRoot 'shared\version.json') -Raw | ConvertFrom-Json).version
$stage = Join-Path $RepoRoot ('artifacts\windows\publish-' + [Guid]::NewGuid())
$destination = Join-Path $RepoRoot ('artifacts\' + $OutputFolder)
$bundle = Join-Path $stage 'QuotaBar-Windows-x64'
New-Item -ItemType Directory -Force -Path $stage, $bundle | Out-Null
Push-Location (Join-Path $RepoRoot 'windows')
try {
    & "$PSScriptRoot\build-native.ps1" -Destination (Join-Path $bundle 'StatusLine')
    & "$PSScriptRoot\write-update-version.ps1" -Path (Join-Path $bundle 'StatusLine\QuotaBar.StatusLine.dll') -Version $version
    & "$PSScriptRoot\write-update-version.ps1" -Path (Join-Path $bundle 'Dashboard\QuotaBar.Dashboard.dll') -Version $version
    $output = Join-Path $stage 'Monitor'
    Invoke-Dotnet @('publish','src/QuotaBar.Monitor/QuotaBar.Monitor.csproj','-c','Release','-r',$Runtime,'--self-contained','true','-p:PublishSingleFile=false','-p:PublishTrimmed=false',"-p:Version=$version",'-m:1','-nodeReuse:false','-p:UseSharedCompilation=false','-o',$output,'--nologo')
    Copy-Item -Path (Join-Path $output '*') -Destination $bundle -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'LICENSE'), (Join-Path $RepoRoot 'NOTICE'), (Join-Path $RepoRoot 'windows\README.md') -Destination $bundle -Force
    $accessKitLicenses = New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'licenses\AccessKit')
    foreach ($name in @('LICENSE-APACHE','LICENSE-MIT','LICENSE.chromium')) {
        Copy-Item -LiteralPath (Join-Path $RepoRoot "vendor\accesskit_windows\$name") -Destination $accessKitLicenses.FullName
    }
    $proxyLicenses = New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'licenses\libcurl')
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'windows\assets\Proxy-LICENSE') -Destination (Join-Path $proxyLicenses.FullName 'LICENSE')
    $zipLicenses = New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'licenses\ZIP')
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'windows\assets\Zip-LICENSE') -Destination (Join-Path $zipLicenses.FullName 'LICENSE')
    $arrowLicense = New-Item -ItemType Directory -Force -Path (Join-Path $bundle 'licenses\Windows Forms')
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'windows\assets\WinForms-LICENSE') -Destination (Join-Path $arrowLicense.FullName 'LICENSE')
    $frameworks = (Get-Content -LiteralPath (Join-Path $bundle 'QuotaBar.Monitor.runtimeconfig.json') -Raw | ConvertFrom-Json).runtimeOptions.includedFrameworks
    foreach ($framework in $frameworks) {
        $packageName = ($framework.name + '.runtime.' + $Runtime).ToLowerInvariant()
        $packageRoot = Join-Path $env:NUGET_PACKAGES ($packageName + '\' + $framework.version)
        $licenses = Join-Path $bundle ('licenses\' + $framework.name)
        New-Item -ItemType Directory -Force -Path $licenses | Out-Null
        Get-ChildItem -LiteralPath $packageRoot -File | Where-Object { $_.Name -like 'LICENSE*' -or $_.Name -like '*THIRD*NOTICE*' } | Copy-Item -Destination $licenses
        if (-not (Get-ChildItem -LiteralPath $licenses -File)) { throw "Runtime license missing: $($framework.name)" }
    }
    $zip = Join-Path $RepoRoot "artifacts\QuotaBar-$version-Windows-x64.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    [IO.Compression.ZipFile]::CreateFromDirectory($bundle, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)
    $artifactRoot = [IO.Path]::GetFullPath((Join-Path $RepoRoot 'artifacts')) + [IO.Path]::DirectorySeparatorChar
    foreach ($target in @($stage, $destination, $bundle)) {
        $resolved = [IO.Path]::GetFullPath($target)
        if (-not $resolved.StartsWith($artifactRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Package path escaped artifacts.' }
        if ((Get-Item -LiteralPath $target -ErrorAction SilentlyContinue).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Package path must not be a link.' }
    }
    if (Test-Path -LiteralPath $destination) { Move-Item -LiteralPath $destination -Destination (Join-Path $stage 'previous') }
    Move-Item -LiteralPath $bundle -Destination $destination
    Get-FileHash -LiteralPath $zip -Algorithm SHA256 | Format-List
    Write-Output "Application: $(Join-Path $destination 'QuotaBar.Monitor.exe')"
    Remove-Item -LiteralPath $stage -Recurse -Force
} finally { Pop-Location }
