$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$toolRoot = Join-Path $repo '.tools\inno-7.1.0'
$compiler = Join-Path $toolRoot 'ISCC.exe'
if (-not (Test-Path -LiteralPath $compiler)) {
    $download = Join-Path $repo '.tools\innosetup-7.1.0-x64.exe'
    $expectedHash = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $download) | Out-Null
    if (-not (Test-Path -LiteralPath $download)) {
        Invoke-WebRequest -Uri 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe' -OutFile $download
    }
    if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedHash) { throw 'Inno Setup download hash mismatch.' }
    $signature = Get-AuthenticodeSignature -LiteralPath $download
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'CN=Pyrsys B\.V\.') { throw 'Inno Setup publisher verification failed.' }
    $process = Start-Process -FilePath $download -ArgumentList @('/PORTABLE=1','/CURRENTUSER','/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-','/NOICONS',('/DIR="' + $toolRoot + '"')) -WindowStyle Hidden -PassThru
    try {
        if (-not $process.WaitForExit(60000)) { $process.Kill($true); throw 'Inno Setup extraction timed out.' }
        if ($process.ExitCode -ne 0) { throw 'Inno Setup extraction failed.' }
    } finally { $process.Dispose() }
}
foreach ($binary in @($compiler, (Join-Path $toolRoot 'ISCmplr.dll'))) {
    $signature = Get-AuthenticodeSignature -LiteralPath $binary
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'CN=Pyrsys B\.V\.') { throw 'Unexpected Inno Setup compiler publisher.' }
}
$version = & $compiler '--version'
if ($LASTEXITCODE -ne 0 -or ($version -join ' ') -notmatch '\b7\.1\.0\b') { throw 'Unexpected Inno Setup compiler version.' }
$compiler
