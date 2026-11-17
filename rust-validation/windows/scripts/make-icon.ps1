. "$PSScriptRoot\common.ps1"
Add-Type -AssemblyName System.Drawing
[xml]$svg = Get-Content -LiteralPath (Join-Path $RepoRoot 'shared\assets\AppMark.svg') -Raw
$folder = Join-Path $RepoRoot 'windows\assets'
New-Item -ItemType Directory -Path $folder -Force | Out-Null
$images = [Collections.Generic.List[byte[]]]::new()
foreach ($size in @(32,48,256)) {
    $bmp = [Drawing.Bitmap]::new($size, $size)
    $graphics = [Drawing.Graphics]::FromImage($bmp)
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.Clear([Drawing.Color]::Transparent)
    $tile = [Drawing.Drawing2D.GraphicsPath]::new()
    $edge = [single]($size * 28 / 512)
    $side = [single]($size * 456 / 512)
    $diameter = [single]($size * 216 / 512)
    $tile.AddArc($edge,$edge,$diameter,$diameter,180,90)
    $tile.AddArc(($edge+$side-$diameter),$edge,$diameter,$diameter,270,90)
    $tile.AddArc(($edge+$side-$diameter),($edge+$side-$diameter),$diameter,$diameter,0,90)
    $tile.AddArc($edge,($edge+$side-$diameter),$diameter,$diameter,90,90)
    $tile.CloseFigure()
    $graphics.FillPath([Drawing.Brushes]::Black,$tile)
    $tile.Dispose()
    $scale = [single]($size / 512)
    $offsetX = [single](($size - 266 * $scale) / 2)
    $offsetY = [single](($size - 282 * $scale) / 2)
    foreach ($rect in $svg.svg.g.rect) {
        $x = [single]$rect.x * $scale + $offsetX
        $y = [single]$rect.y * $scale + $offsetY
        $w = [single]$rect.width * $scale
        $h = [single]$rect.height * $scale
        $diameter = [single]$rect.rx * $scale * 2
        $path = [Drawing.Drawing2D.GraphicsPath]::new()
        $path.AddArc($x,$y,$diameter,$diameter,180,90)
        $path.AddArc(($x+$w-$diameter),$y,$diameter,$diameter,270,90)
        $path.AddArc(($x+$w-$diameter),($y+$h-$diameter),$diameter,$diameter,0,90)
        $path.AddArc($x,($y+$h-$diameter),$diameter,$diameter,90,90)
        $path.CloseFigure()
        $graphics.FillPath([Drawing.Brushes]::White,$path)
        $path.Dispose()
    }
    $stream = [IO.MemoryStream]::new()
    $bmp.Save($stream,[Drawing.Imaging.ImageFormat]::Png)
    $images.Add($stream.ToArray())
    $stream.Dispose(); $graphics.Dispose(); $bmp.Dispose()
}
$writer = [IO.BinaryWriter]::new([IO.File]::Create((Join-Path $folder 'QuotaBar.ico')))
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]3)
    $offset = 6 + 16 * 3
    for ($i=0; $i -lt 3; $i++) {
        $size = @(32,48,0)[$i]
        $writer.Write([byte]$size); $writer.Write([byte]$size); $writer.Write([uint16]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32); $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
        $offset += $images[$i].Length
    }
    foreach ($bytes in $images) { $writer.Write($bytes) }
} finally { $writer.Dispose() }
