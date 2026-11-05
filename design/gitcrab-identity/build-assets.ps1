$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$out = $PSScriptRoot
$ink = '#111318'
$blue = '#668BFF'
$charcoal = '#292D36'
$paper = '#FFFFFF'
$claw = '22,8 42,8 23,27 23,36 45,34 28,51 28,57 18,57 6,45 6,25'
$shell = '64,40 88,53 88,56 64,70 40,56 40,53'
$leg = '20,65 34,65 60,80 60,94 24,87 4,104 4,84'

function New-PolyPath([string]$points, [bool]$mirror = $false) {
  $p = [System.Drawing.Drawing2D.GraphicsPath]::new()
  $arr = [System.Drawing.PointF[]]@($points.Split(' ') | ForEach-Object {
    $xy = $_.Split(','); $x = [single]$xy[0]
    if ($mirror) { $x = 128 - $x }
    [System.Drawing.PointF]::new($x, [single]$xy[1])
  })
  $p.AddPolygon($arr)
  return ,$p
}
$parts = @((New-PolyPath $claw), (New-PolyPath $claw $true), (New-PolyPath $shell), (New-PolyPath $leg), (New-PolyPath $leg $true))
$font = [System.Drawing.FontFamily]::new('Bahnschrift')
$word = [System.Drawing.Drawing2D.GraphicsPath]::new()
$word.FillMode = [System.Drawing.Drawing2D.FillMode]::Winding
$word.AddString('GitCrab', $font, [int][System.Drawing.FontStyle]::Regular, 88, [System.Drawing.PointF]::new(0,0), [System.Drawing.StringFormat]::GenericTypographic)
$bounds = $word.GetBounds()
$move = [System.Drawing.Drawing2D.Matrix]::new()
$move.Translate(-$bounds.X, -$bounds.Y)
$word.Transform($move)
$wordWidth = $word.GetBounds().Width
$wordHeight = $word.GetBounds().Height
$lockupWidth = 128 + 28 + $wordWidth

function F($v) { return ([double]$v).ToString('0.###', [Globalization.CultureInfo]::InvariantCulture) }
function Path-Data($p) {
  $pts = $p.PathPoints; $types = $p.PathTypes
  $s = [Text.StringBuilder]::new()
  for ($i=0; $i -lt $pts.Length; $i++) {
    $type = $types[$i] -band 7
    if ($type -eq 0) { [void]$s.Append("M$(F $pts[$i].X) $(F $pts[$i].Y)") }
    elseif ($type -eq 1) { [void]$s.Append("L$(F $pts[$i].X) $(F $pts[$i].Y)") }
    elseif ($type -eq 3) {
      [void]$s.Append("C$(F $pts[$i].X) $(F $pts[$i].Y) $(F $pts[$i+1].X) $(F $pts[$i+1].Y) $(F $pts[$i+2].X) $(F $pts[$i+2].Y)")
      $i += 2
    }
    if (($types[$i] -band 128) -ne 0) { [void]$s.Append('Z') }
  }
  return $s.ToString()
}
function Symbol-Svg($main = $ink, $accent = $blue) {
  $s = ''
  for ($i=0; $i -lt $parts.Count; $i++) {
    $c = $main; if ($i -eq 1) { $c = $accent }
    $s += "<path fill='$c' d='$(Path-Data $parts[$i])'/>"
  }
  return $s
}
function Lockup-Svg($main = $ink, $accent = $blue) {
  $y = (112 - $wordHeight) / 2
  return "$(Symbol-Svg $main $accent)<path fill='$main' transform='translate(156 $(F $y))' d='$(Path-Data $word)'/>"
}
function Save-Svg($name, $w, $h, $content) {
  $svg = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 $w $h' width='$w' height='$h' role='img' aria-label='GitCrab $name'><title>GitCrab $name</title>$content</svg>"
  [IO.File]::WriteAllText((Join-Path $out "$name.svg"), $svg)
}
function Brush($hex) { return [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($hex)) }
function Draw-Symbol($g, $x, $y, $scale, $main = $ink, $accent = $blue) {
  $state = $g.Save(); $g.TranslateTransform([single]$x,[single]$y); $g.ScaleTransform([single]$scale,[single]$scale)
  for ($i=0; $i -lt $parts.Count; $i++) {
    $c = $main; if ($i -eq 1) { $c = $accent }
    $b = Brush $c; $g.FillPath($b, $parts[$i]); $b.Dispose()
  }
  $g.Restore($state)
}
function Draw-Lockup($g, $x, $y, $scale, $main = $ink, $accent = $blue) {
  Draw-Symbol $g $x $y $scale $main $accent
  $state = $g.Save(); $g.TranslateTransform([single]($x+156*$scale),[single]($y+((112-$wordHeight)/2)*$scale)); $g.ScaleTransform([single]$scale,[single]$scale)
  $b = Brush $main; $g.FillPath($b,$word); $b.Dispose(); $g.Restore($state)
}
function New-Canvas($width, $height, $name, $draw) {
  $bmp = [Drawing.Bitmap]::new([int]$width,[int]$height,[Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g.CompositingQuality = [Drawing.Drawing2D.CompositingQuality]::HighQuality
  & $draw $g
  $bmp.Save((Join-Path $out "$name.png"),[Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}
function Rect($g,$x,$y,$w,$h,$c) { $b=Brush $c; $g.FillRectangle($b,[single]$x,[single]$y,[single]$w,[single]$h); $b.Dispose() }
function Label($g,$text,$x,$y,$size=16,$c='#6A6E78') {
  $f=[Drawing.Font]::new('Segoe UI',[single]$size,[Drawing.FontStyle]::Regular,[Drawing.GraphicsUnit]::Pixel)
  $b=Brush $c; $g.DrawString($text,$f,$b,[single]$x,[single]$y); $b.Dispose(); $f.Dispose()
}
function Rounded-Path($x,$y,$w,$h,$r) {
  $p=[Drawing.Drawing2D.GraphicsPath]::new(); $d=$r*2
  $p.AddArc($x,$y,$d,$d,180,90); $p.AddArc($x+$w-$d,$y,$d,$d,270,90)
  $p.AddArc($x+$w-$d,$y+$h-$d,$d,$d,0,90); $p.AddArc($x,$y+$h-$d,$d,$d,90,90); $p.CloseFigure()
  return ,$p
}
function Draw-App($g,$x,$y,$size) {
  $p=Rounded-Path $x $y $size $size ($size*.22); $b=Brush $ink; $g.FillPath($b,$p); $b.Dispose(); $p.Dispose()
  $s=$size*.72/120
  Draw-Symbol $g ($x+$size*.14-4*$s) ($y+$size/2-56*$s) $s $paper $blue
}

foreach ($variant in @('color','dark','mono','white')) {
  $main=$ink; $accent=$blue
  if ($variant -eq 'dark') { $main=$paper }
  if ($variant -eq 'mono') { $accent=$ink }
  if ($variant -eq 'white') { $main=$paper; $accent=$paper }
  Save-Svg "symbol-$variant" 160 160 "<g transform='translate(16 24)'>$(Symbol-Svg $main $accent)</g>"
  New-Canvas 1280 1280 "symbol-$variant" { param($g) Draw-Symbol $g 128 192 8 $main $accent }
  $w=[math]::Ceiling($lockupWidth+64)
  Save-Svg "lockup-$variant" $w 176 "<g transform='translate(32 32)'>$(Lockup-Svg $main $accent)</g>"
  New-Canvas ($w*3) 528 "lockup-$variant" { param($g) Draw-Lockup $g 96 96 3 $main $accent }
}
foreach ($size in @(16,24,32,64,128,256,512,1024)) {
  New-Canvas $size $size "app-icon-$size" { param($g) Draw-App $g 0 0 $size }
}
foreach ($size in @(16,24,32)) {
  $s=$size/128
  New-Canvas $size $size "toolbar-color-$size" { param($g) Draw-Symbol $g 0 ($size/2-56*$s) $s }
  New-Canvas $size $size "toolbar-mono-$size" { param($g) Draw-Symbol $g 0 ($size/2-56*$s) $s $ink $ink }
  New-Canvas $size $size "toolbar-white-$size" { param($g) Draw-Symbol $g 0 ($size/2-56*$s) $s $paper $paper }
}
Save-Svg 'app-icon' 1024 1024 "<rect width='1024' height='1024' rx='225.28' fill='$ink'/><g transform='translate(118.784 167.936) scale(6.144)'>$(Symbol-Svg $paper $blue)</g>"

New-Canvas 1600 1120 'identity-board' {
  param($g)
  $g.Clear([Drawing.Color]::White)
  Label $g 'GITCRAB / VISUAL IDENTITY' 64 36 18 $ink
  Label $g '03 / SPLITCLAW' 1355 36 16
  Rect $g 64 82 1472 1 '#DEE0E6'
  $scale=1120/$lockupWidth
  Draw-Lockup $g 230 128 $scale
  Label $g 'Two pincers. One branching space.' 64 426 23 $ink
  Label $g 'Faceted geometry / Bahnschrift wordmark / Flat spot color' 64 465 17
  $cardY=526; $cardW=474
  Rect $g 64 $cardY $cardW 216 '#F5F6F8'
  Rect $g 563 $cardY $cardW 216 $charcoal
  Rect $g 1062 $cardY $cardW 216 '#F5F6F8'
  Label $g 'LIGHT' 86 545 14
  Label $g 'DARK' 585 545 14 '#D8DCE6'
  Label $g 'MONOCHROME' 1084 545 14
  $sc=410/$lockupWidth
  Draw-Lockup $g 94 598 $sc
  Draw-Lockup $g 593 598 $sc $paper $blue
  Draw-Lockup $g 1092 598 $sc $ink $ink
  Label $g 'DESKTOP APP' 64 790 14
  Draw-App $g 64 832 176
  Draw-App $g 270 888 64
  Draw-App $g 362 920 32
  Label $g '64' 288 972 14
  Label $g '32' 368 972 14
  Label $g 'TOOLBAR / ACTUAL PIXEL SIZES' 500 790 14
  Rect $g 500 840 500 92 '#F5F6F8'
  $xs=@(548,694,840); $sizes=@(32,24,16)
  for($i=0;$i -lt 3;$i++) {
    $size=$sizes[$i]; $s=$size/128
    Draw-Symbol $g $xs[$i] (886-$size/2+($size/2-56*$s)) $s
    Label $g "$size px" ($xs[$i]-2) 950 14
  }
  Label $g 'PALETTE' 1080 790 14
  $colors=@($ink,$charcoal,$blue)
  for($i=0;$i -lt 3;$i++) {
    $x=1080+$i*156; Rect $g $x 842 132 66 $colors[$i]
    Label $g $colors[$i] $x 925 16 $ink
  }
  Rect $g 64 1044 1472 1 '#DEE0E6'
  Label $g 'GITCRAB' 64 1065 14 $ink
  Label $g 'One silhouette across color, monochrome and small UI sizes.' 882 1065 16
}

Write-Output "Created identity assets in $out"
Write-Output "Wordmark font: $($font.Name); outlined in all SVG lockups"
