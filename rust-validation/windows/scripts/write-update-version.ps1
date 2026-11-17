param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][version]$Version)
. "$PSScriptRoot\common.ps1"

# Released Windows updaters read the assembly identity. Intermediate Rust
# previews also read the Win32 file-version resource from the companion DLL.
$name = [IO.Path]::GetFileNameWithoutExtension($Path)
$version4 = [version]::new($Version.Major, $Version.Minor, $Version.Build, 0)
$work = Join-Path $RepoRoot ('artifacts\windows\component-version-' + [Guid]::NewGuid().ToString('N'))
$output = Join-Path $work 'publish'
Invoke-Dotnet @('build', (Join-Path $PSScriptRoot 'ComponentVersion.csproj'), '-c', 'Release',
    '--artifacts-path', $work, '-o', $output, "-p:AssemblyName=$name", "-p:Version=$Version",
    "-p:AssemblyVersion=$version4", "-p:FileVersion=$version4", '-p:UseSharedCompilation=false',
    '-m:1', '-nodeReuse:false', '--nologo', '--verbosity', 'quiet')
Copy-Item -LiteralPath (Join-Path $output "$name.dll") -Destination $Path -Force
