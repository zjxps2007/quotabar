# Run in a dedicated disposable CI checkout, separate from production package jobs.
. "$PSScriptRoot\common.ps1"
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This fixture test requires an isolated GitHub Actions runner.' }
$keyPath = Join-Path $RepoRoot 'artifacts\updater-test-private.pem'
$publicPath = Join-Path $RepoRoot 'shared\assets\UpdatePublicKey.pem'
$versionPath = Join-Path $RepoRoot 'shared\version.json'
$originalPublic = [IO.File]::ReadAllBytes($publicPath)
$originalVersion = [IO.File]::ReadAllBytes($versionPath)
New-Item -ItemType Directory -Force -Path (Join-Path $RepoRoot 'artifacts') | Out-Null
$key = [Security.Cryptography.ECDsa]::Create()
try {
    $key.GenerateKey([Security.Cryptography.ECCurve]::CreateFromFriendlyName('nistP256'))
    [IO.File]::WriteAllText($keyPath, $key.ExportPkcs8PrivateKeyPem())
    [IO.File]::WriteAllText($publicPath, $key.ExportSubjectPublicKeyInfoPem())
    [IO.File]::WriteAllText($versionPath, '{"version":"0.1.0","build":6}')
    Invoke-Dotnet @('build', (Join-Path $RepoRoot 'windows\tests\QuotaBar.Tests\QuotaBar.Tests.csproj'), '-c', 'Release', '--nologo', '-m:1', '-nodeReuse:false', '-p:UseSharedCompilation=false')
    & "$PSScriptRoot\package.ps1" -OutputFolder 'Updater-Old'
    & "$PSScriptRoot\package-installer.ps1"
    [IO.File]::WriteAllText($versionPath, '{"version":"0.1.1","build":7}')
    & "$PSScriptRoot\package.ps1" -OutputFolder 'Updater-New'
    & "$PSScriptRoot\package-installer.ps1"
    $tests = Join-Path $RepoRoot 'windows\tests\QuotaBar.Tests\bin\Release\net10.0-windows\QuotaBar.Tests.dll'
    $old = Join-Path $RepoRoot 'artifacts\Updater-Old'
    $oldSetup = Join-Path $RepoRoot 'artifacts\QuotaBar-0.1.0-Windows-x64-Setup.exe'
    foreach ($mode in @('portable', 'rollback', 'installed')) {
        $suffix = if ($mode -eq 'installed') { '-Setup.exe' } else { '.zip' }
        $asset = Join-Path $RepoRoot ('artifacts\QuotaBar-0.1.1-Windows-x64' + $suffix)
        Invoke-Dotnet @($tests, '--update-integration', $mode, $old, $oldSetup, $asset, $keyPath)
    }
} finally {
    if ($null -ne $key) { $key.Dispose() }
    [IO.File]::WriteAllBytes($publicPath, $originalPublic)
    [IO.File]::WriteAllBytes($versionPath, $originalVersion)
    Remove-Item -LiteralPath $keyPath -Force -ErrorAction SilentlyContinue
}
