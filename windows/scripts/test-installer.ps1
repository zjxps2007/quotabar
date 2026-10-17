param([string]$InstallerPath)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$metadata = Get-Content -LiteralPath (Join-Path $repo 'artifacts\windows\installer-build-info.json') -Raw | ConvertFrom-Json
if (-not $InstallerPath) { $InstallerPath = Join-Path $repo ('artifacts\' + $metadata.installer.name) }
$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath).Path
if ((Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $metadata.installer.sha256) { throw 'Installer differs from build metadata.' }
$registryPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\QuotaBar.Windows_is1'
if (Test-Path -LiteralPath $registryPath) { throw 'An existing QuotaBar installation is registered. Use a clean Windows account for installer testing.' }
$artifactRoot = [IO.Path]::GetFullPath((Join-Path $repo 'artifacts')) + [IO.Path]::DirectorySeparatorChar
$testRoot = Join-Path $artifactRoot ('windows\installer-test-' + [Guid]::NewGuid().ToString('N'))
$destination = Join-Path $testRoot '한글 설치 경로'
if (-not [IO.Path]::GetFullPath($destination).StartsWith($artifactRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Test path escaped artifacts.' }
New-Item -ItemType Directory -Path $testRoot | Out-Null
$group = 'QuotaBar Installer Test ' + [Guid]::NewGuid().ToString('N')
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) ($group + '\QuotaBar.lnk')
$payload = Get-Content -LiteralPath (Join-Path $repo 'artifacts\windows\installer-payload-path.txt') -Raw
$files = @(Get-ChildItem -LiteralPath $payload -Recurse -File | ForEach-Object {
    [pscustomobject]@{ Relative = [IO.Path]::GetRelativePath($payload, $_.FullName); Hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
})
function Invoke-InstallerProcess([string]$Executable, [string[]]$Arguments) {
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(60000)) { $process.Kill($true); throw 'Installer process timed out.' }
        if ($process.ExitCode -ne 0) { throw "Installer process failed: $($process.ExitCode)" }
    } finally { $process.Dispose() }
}
function Remove-TestInstallation {
    $uninstaller = Join-Path $destination 'unins000.exe'
    if (-not (Test-Path -LiteralPath $uninstaller)) { return }
    $registration = Get-ItemProperty -LiteralPath $registryPath -ErrorAction SilentlyContinue
    if ($registration -and $registration.InstallLocation.TrimEnd('\') -ne $destination) { throw 'Refusing to uninstall a different installation.' }
    Invoke-InstallerProcess $uninstaller @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',('/LOG="' + (Join-Path $testRoot 'uninstall.log') + '"'))
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ((Test-Path -LiteralPath $registryPath) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
}
$previousHelper = $env:QUOTABAR_TEST_HELPER
try {
    foreach ($phase in @('install', 'reinstall')) {
        Invoke-InstallerProcess $InstallerPath @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-','/LANG=korean','/TASKS=',('/DIR="' + $destination + '"'),('/GROUP="' + $group + '"'),('/LOG="' + (Join-Path $testRoot ($phase + '.log')) + '"'))
        $registration = Get-ItemProperty -LiteralPath $registryPath
        if ($registration.InstallLocation.TrimEnd('\') -ne $destination -or $registration.DisplayVersion -ne $metadata.version) { throw 'Installed application registration mismatch.' }
        if (-not (Test-Path -LiteralPath $shortcutPath)) { throw 'Start menu shortcut missing.' }
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($shortcutPath)
        try { if ($shortcut.TargetPath -ne (Join-Path $destination 'QuotaBar.Monitor.exe')) { throw 'Start menu shortcut points to the wrong executable.' } }
        finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut); [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) }
        foreach ($file in $files) {
            if ((Get-FileHash -LiteralPath (Join-Path $destination $file.Relative) -Algorithm SHA256).Hash -ne $file.Hash) { throw "Installed file mismatch: $($file.Relative)" }
        }
        $sentinel = Join-Path $destination 'user-created-file.txt'
        if ($phase -eq 'install') { [IO.File]::WriteAllText($sentinel, 'preserve user content') }
        elseif ((Get-Content -LiteralPath $sentinel -Raw) -ne 'preserve user content') { throw 'Reinstallation changed an unmanaged file.' }
        Write-Output "$phase passed: $($files.Count) application files verified."
    }
    $env:QUOTABAR_TEST_HELPER = Join-Path $destination 'StatusLine\QuotaBar.StatusLine.exe'
    & "$PSScriptRoot\test.ps1" -NoBuild
    & "$PSScriptRoot\smoke.ps1" -ApplicationDirectory $destination
    $smoke = Get-Content -LiteralPath (Join-Path $repo 'artifacts\windows\packaged-probe.json') -Raw | ConvertFrom-Json
    if (-not $smoke.passed -or -not $smoke.parentCrashCleanup) { throw 'Installed application smoke failed.' }
    Remove-TestInstallation
    if ((Test-Path -LiteralPath $registryPath) -or (Test-Path -LiteralPath $shortcutPath)) { throw 'Uninstall registration or shortcut was not removed.' }
    foreach ($file in $files) { if (Test-Path -LiteralPath (Join-Path $destination $file.Relative)) { throw "Installed file remained after uninstall: $($file.Relative)" } }
    if ((Get-Content -LiteralPath $sentinel -Raw) -ne 'preserve user content') { throw 'Uninstaller removed an unmanaged file.' }
    [ordered]@{ passed = $true; version = $metadata.version; installer_sha256 = $metadata.installer.sha256; install = $true; same_version_reinstall = $true; uninstall = $true; application_files_verified = $files.Count; unicode_and_spaces_path = $true; start_menu_shortcut = $true; uninstall_registration_removed = $true; unmanaged_file_preserved = $true; packaged_smoke = $true; integration_passed = 30; integration_failed = 0 } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $repo 'artifacts\windows\installer-validation.json') -Encoding utf8
    Write-Output 'Installer validation passed: install, reinstall, runtime, uninstall.'
} finally {
    $env:QUOTABAR_TEST_HELPER = $previousHelper
    if (Test-Path -LiteralPath $registryPath) { Remove-TestInstallation }
}
