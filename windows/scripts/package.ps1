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
    foreach ($project in @('Monitor','Dashboard','StatusLine')) {
        $output = Join-Path $stage $project
        Invoke-Dotnet @('publish',"src/QuotaBar.$project/QuotaBar.$project.csproj",'-c','Release','-r',$Runtime,'--self-contained','true','-p:PublishSingleFile=false','-p:PublishTrimmed=false',"-p:Version=$version",'-m:1','-nodeReuse:false','-p:UseSharedCompilation=false','-o',$output,'--nologo')
        # WPF and Core carry different WindowsBase.dll files. Never merge these runtimes.
        $component = if ($project -eq 'Monitor') { $bundle } else { Join-Path $bundle $project }
        New-Item -ItemType Directory -Force -Path $component | Out-Null
        Copy-Item -Path (Join-Path $output '*') -Destination $component -Recurse -Force
    }
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'LICENSE'), (Join-Path $RepoRoot 'NOTICE'), (Join-Path $RepoRoot 'windows\README.md') -Destination $bundle -Force
    $frameworks = (Get-Content -LiteralPath (Join-Path $bundle 'Dashboard\QuotaBar.Dashboard.runtimeconfig.json') -Raw | ConvertFrom-Json).runtimeOptions.includedFrameworks
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
