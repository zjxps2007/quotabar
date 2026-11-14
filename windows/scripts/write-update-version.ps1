using namespace System.Reflection
using namespace System.Reflection.Metadata
using namespace System.Reflection.Metadata.Ecma335
using namespace System.Reflection.PortableExecutable

param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][version]$Version)
$ErrorActionPreference = 'Stop'

# Released Windows updaters call AssemblyName.GetAssemblyName on each component.
# The native process uses its EXE version resource. This DLL contains only metadata.
$name = [IO.Path]::GetFileNameWithoutExtension($Path)
$version4 = [version]::new($Version.Major, $Version.Minor, $Version.Build, 0)
$identity = [Text.Encoding]::UTF8.GetBytes("$name/$version4")
$hash = [Security.Cryptography.SHA256]::HashData($identity)
$id = [Guid]::new([byte[]]$hash[0..15])
$metadata = [MetadataBuilder]::new()
$nilGuid = [GuidHandle]::new()
$nilString = [StringHandle]::new()
$nilBlob = [BlobHandle]::new()
$null = $metadata.AddModule(0, $metadata.GetOrAddString("$name.dll"), $metadata.GetOrAddGuid($id), $nilGuid, $nilGuid)
$null = $metadata.AddAssembly($metadata.GetOrAddString($name), $version4, $nilString, $nilBlob,
    [AssemblyFlags]0, [AssemblyHashAlgorithm]::Sha256)
$null = $metadata.AddTypeDefinition([TypeAttributes]::NotPublic, $nilString, $metadata.GetOrAddString('<Module>'),
    [EntityHandle]::new(), [MetadataTokens]::FieldDefinitionHandle(1), [MetadataTokens]::MethodDefinitionHandle(1))
$writer = [ManagedPEBuilder]::new([PEHeaderBuilder]::CreateLibraryHeader(), [MetadataRootBuilder]::new($metadata), [BlobBuilder]::new())
$image = [BlobBuilder]::new()
$null = $writer.Serialize($image)
[IO.File]::WriteAllBytes($Path, $image.ToArray())
