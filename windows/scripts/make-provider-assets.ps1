# Run with Windows PowerShell (powershell.exe -NoProfile -File ...).
# Rasterize the shared SVG at build time so the resident WinForms monitor never loads WPF.
$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
Add-Type -AssemblyName PresentationCore, WindowsBase
[xml]$svg = Get-Content -LiteralPath (Join-Path $repoRoot 'shared\assets\ClaudeLogo.svg') -Raw
$visual = New-Object System.Windows.Media.DrawingVisual
$drawing = $visual.RenderOpen()
try {
    $drawing.PushTransform((New-Object System.Windows.Media.ScaleTransform(4, 4)))
    $brush = [Windows.Media.BrushConverter]::new().ConvertFromInvariantString($svg.svg.path.fill)
    $drawing.DrawGeometry($brush, $null, [Windows.Media.Geometry]::Parse($svg.svg.path.d))
    $drawing.Pop()
} finally { $drawing.Close() }
$bitmap = [Windows.Media.Imaging.RenderTargetBitmap]::new(96, 96, 96, 96, [Windows.Media.PixelFormats]::Pbgra32)
$bitmap.Render($visual)
$encoder = New-Object System.Windows.Media.Imaging.PngBitmapEncoder
$encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($bitmap))
$file = [IO.File]::Create((Join-Path $repoRoot 'windows\assets\ClaudeLogo.png'))
try { $encoder.Save($file) } finally { $file.Dispose() }
