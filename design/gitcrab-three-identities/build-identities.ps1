$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$root=$PSScriptRoot
$ink='#111318'; $blue='#668BFF'; $white='#FFFFFF'; $charcoal='#292D36'

function Brush($c) { return [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml($c)) }
function Poly([string]$s,[bool]$mirror=$false) {
  $p=[Drawing.Drawing2D.GraphicsPath]::new()
  $pts=[Drawing.PointF[]]@($s.Split(' ') | ForEach-Object { $v=$_.Split(','); $x=[single]$v[0]; if($mirror){$x=128-$x}; [Drawing.PointF]::new($x,[single]$v[1]) })
  $p.AddPolygon($pts); return ,$p
}
function Ring($x,$y,$r,$hole) {
  $p=[Drawing.Drawing2D.GraphicsPath]::new(); $p.AddEllipse([single]($x-$r),[single]($y-$r),[single]($r*2),[single]($r*2)); $p.AddEllipse([single]($x-$hole),[single]($y-$hole),[single]($hole*2),[single]($hole*2)); return ,$p
}
function Part($p,$accent=$false) { return @{Path=$p; Accent=$accent} }
function WordPath($family,$style,$text,$size) {
  $p=[Drawing.Drawing2D.GraphicsPath]::new(); $p.FillMode=[Drawing.Drawing2D.FillMode]::Winding
  $f=[Drawing.FontFamily]::new($family)
  $p.AddString($text,$f,[int]$style,[single]$size,[Drawing.PointF]::new(0,0),[Drawing.StringFormat]::GenericTypographic)
  $b=$p.GetBounds(); $m=[Drawing.Drawing2D.Matrix]::new(); $m.Translate(-$b.X,-$b.Y); $p.Transform($m); $m.Dispose(); $f.Dispose(); return ,$p
}
function Set-Profile($id) {
  $script:id=$id
  if($id -eq '01-forkshell') {
    $body=Poly '44,42 84,42 102,54 94,70 82,82 46,82 34,70 26,54'
    $cut=Poly '51,51 58,51 58,57 64,62 70,57 70,51 77,51 77,61 68,69 68,82 60,82 60,69 51,61'
    $body.AddPath($cut,$false)
    $claw='10,17 37,4 24,24 24,31 44,19 41,35 24,47 14,40 6,32'
    $arm='22,37 37,54 28,61 14,43'
    $legs='27,55 42,60 36,68 18,70 4,87 6,65'
    $lower='38,69 48,76 28,82 16,98 17,80'
    $script:parts=@((Part (Poly $arm)),(Part (Poly $arm $true)),(Part $body),(Part (Poly $legs)),(Part (Poly $legs $true)),(Part (Poly $lower)),(Part (Poly $lower $true)),(Part (Poly $claw)),(Part (Poly $claw $true) $true))
    $script:name='Forkshell'; $script:num='01'; $script:theme='Own your next commit.'
    $script:family='Segoe UI'; $script:weight=[Drawing.FontStyle]::Bold
    $script:fontLabel='Segoe UI Bold / Consolas'; $script:tone='Bold. Direct. In control.'
    $script:canvas='#F4F5F7'; $script:rule='#DBDEE5'; $script:muted='#656B77'
    $script:headline=@('Own your','next commit.')
  } elseif($id -eq '02-branchline') {
    $left=Poly '15,22 25,28 17,40 36,59 29,68 5,45 5,35'
    $branch=Poly '29,60 49,40 84,40 84,52 55,52 38,69'
    $right=Poly '84,40 94,30 94,21 106,21 106,35 89,52 84,52'
    $body=Poly '29,60 42,60 54,72 82,72 94,60 112,60 124,72 124,96 112,84 108,74 99,74 88,84 50,84 36,72 28,78 20,92 12,100 12,82 24,68'
    $leg=Poly '15,60 33,60 33,72 21,72 4,89 4,74'
    $script:parts=@((Part $left),(Part $branch),(Part $body),(Part $leg),(Part $right $true),(Part (Ring 21 14 11 4.5)),(Part (Ring 100 14 11 4.5) $true))
    $script:name='Branchline'; $script:num='02'; $script:theme='Every branch. A clear path.'
    $script:family='Segoe UI'; $script:weight=[Drawing.FontStyle]::Regular
    $script:fontLabel='Segoe UI Regular / Consolas'; $script:tone='Open. Calm. Connected.'
    $script:canvas='#F6F8FC'; $script:rule='#DCE2ED'; $script:muted='#626D80'
    $script:headline=@('Every branch.','A clear path.')
  } else {
    $claw='22,8 42,8 23,27 23,36 45,34 28,51 28,57 18,57 6,45 6,25'
    $leg='20,65 34,65 60,80 60,94 24,87 4,104 4,84'
    $script:parts=@((Part (Poly $claw)),(Part (Poly $claw $true) $true),(Part (Poly '64,40 88,53 88,56 64,70 40,56 40,53')),(Part (Poly $leg)),(Part (Poly $leg $true)))
    $script:name='Splitclaw'; $script:num='03'; $script:theme='Precision in every change.'
    $script:family='Bahnschrift'; $script:weight=[Drawing.FontStyle]::Regular
    $script:fontLabel='Bahnschrift / Consolas'; $script:tone='Precise. Modular. Assured.'
    $script:canvas='#F3F4F6'; $script:rule='#D8DBE2'; $script:muted='#676D78'
    $script:headline=@('Precision in','every change.')
  }
  $script:word=WordPath $family $weight 'GitCrab' 86
  $script:wordW=$word.GetBounds().Width; $script:wordH=$word.GetBounds().Height
  $script:lockW=156+$wordW
}
function Num($v) { return ([double]$v).ToString('0.###',[Globalization.CultureInfo]::InvariantCulture) }
function PathData($p) {
  $s=[Text.StringBuilder]::new(); $pts=$p.PathPoints; $ts=$p.PathTypes
  for($i=0;$i -lt $pts.Length;$i++) {
    $t=$ts[$i] -band 7
    if($t -eq 0){[void]$s.Append("M$(Num $pts[$i].X) $(Num $pts[$i].Y)")}
    elseif($t -eq 1){[void]$s.Append("L$(Num $pts[$i].X) $(Num $pts[$i].Y)")}
    elseif($t -eq 3){[void]$s.Append("C$(Num $pts[$i].X) $(Num $pts[$i].Y) $(Num $pts[$i+1].X) $(Num $pts[$i+1].Y) $(Num $pts[$i+2].X) $(Num $pts[$i+2].Y)");$i+=2}
    if(($ts[$i] -band 128) -ne 0){[void]$s.Append('Z')}
  };return $s.ToString()
}
function SymbolSvg($main=$ink,$accent=$blue) {
  $s='';foreach($part in $parts){$c=$main;if($part.Accent){$c=$accent};$s+="<path fill='$c' fill-rule='evenodd' d='$(PathData $part.Path)'/>"}; return $s
}
function LockSvg($main=$ink,$accent=$blue) { return "$(SymbolSvg $main $accent)<path fill='$main' fill-rule='nonzero' transform='translate(156 $(Num ((112-$wordH)/2)))' d='$(PathData $word)'/>" }
function SaveSvg($file,$w,$h,$content) { [IO.File]::WriteAllText($file,"<svg xmlns='http://www.w3.org/2000/svg' width='$w' height='$h' viewBox='0 0 $w $h' role='img' aria-label='GitCrab $name'><title>GitCrab / $name</title>$content</svg>") }
function Render($file,$w,$h,$draw) {
  $bmp=[Drawing.Bitmap]::new([int]$w,[int]$h,[Drawing.Imaging.PixelFormat]::Format32bppArgb);$g=[Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode=[Drawing.Drawing2D.SmoothingMode]::AntiAlias;$g.PixelOffsetMode=[Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  & $draw $g
  $bmp.Save($file,[Drawing.Imaging.ImageFormat]::Png);$g.Dispose();$bmp.Dispose()
}
function Rect($g,$x,$y,$w,$h,$c) { $b=Brush $c;$g.FillRectangle($b,[single]$x,[single]$y,[single]$w,[single]$h);$b.Dispose() }
function Line($g,$x1,$y1,$x2,$y2,$c,$width=1) { $p=[Drawing.Pen]::new([Drawing.ColorTranslator]::FromHtml($c),[single]$width);$g.DrawLine($p,[single]$x1,[single]$y1,[single]$x2,[single]$y2);$p.Dispose() }
function Dot($g,$x,$y,$r,$c) { $b=Brush $c;$g.FillEllipse($b,[single]($x-$r),[single]($y-$r),[single]($r*2),[single]($r*2));$b.Dispose() }
function Text($g,$str,$x,$y,$size=18,$c=$ink,$font='Segoe UI',$style=[Drawing.FontStyle]::Regular) {
  $f=[Drawing.Font]::new($font,[single]$size,$style,[Drawing.GraphicsUnit]::Pixel);$b=Brush $c;$g.DrawString($str,$f,$b,[single]$x,[single]$y,[Drawing.StringFormat]::GenericTypographic);$b.Dispose();$f.Dispose()
}
function Head($g,$str,$x,$y,$size,$c=$ink) { $p=WordPath $family $weight $str $size;$s=$g.Save();$g.TranslateTransform([single]$x,[single]$y);$b=Brush $c;$g.FillPath($b,$p);$b.Dispose();$g.Restore($s);$p.Dispose() }
function Symbol($g,$x,$y,$s,$main=$ink,$accent=$blue) {
  $state=$g.Save();$g.TranslateTransform([single]$x,[single]$y);$g.ScaleTransform([single]$s,[single]$s)
  foreach($part in $parts){$c=$main;if($part.Accent){$c=$accent};$b=Brush $c;$g.FillPath($b,$part.Path);$b.Dispose()};$g.Restore($state)
}
function Lockup($g,$x,$y,$s,$main=$ink,$accent=$blue) {
  Symbol $g $x $y $s $main $accent
  $state=$g.Save();$g.TranslateTransform([single]($x+156*$s),[single]($y+(112-$wordH)/2*$s));$g.ScaleTransform([single]$s,[single]$s);$b=Brush $main;$g.FillPath($b,$word);$b.Dispose();$g.Restore($state)
}
function App($g,$x,$y,$size) {
  $r=$size*.21;$d=$r*2;$p=[Drawing.Drawing2D.GraphicsPath]::new()
  $p.AddArc($x,$y,$d,$d,180,90);$p.AddArc($x+$size-$d,$y,$d,$d,270,90);$p.AddArc($x+$size-$d,$y+$size-$d,$d,$d,0,90);$p.AddArc($x,$y+$size-$d,$d,$d,90,90);$p.CloseFigure()
  $b=Brush $ink;$g.FillPath($b,$p);$b.Dispose();$p.Dispose();$s=$size*.72/120
  Symbol $g ($x+$size*.14-4*$s) ($y+$size/2-53*$s) $s $white $blue
}
function Motif($g,$x,$y,$w,$h,$color=$blue) {
  $st=$g.Save();$g.SetClip([Drawing.RectangleF]::new($x,$y,$w,$h));$g.TranslateTransform([single]$x,[single]$y)
  if($num -eq '01') {
    for($i=0;$i -lt 4;$i++) { $off=$i*80;Line $g $off $h ($off+$h) 0 $color 18 }
  } elseif($num -eq '02') {
    $p=[Drawing.Drawing2D.GraphicsPath]::new();$p.AddBezier(10,$h-10,10,30,$w-10,$h-30,$w-10,10)
    $pen=[Drawing.Pen]::new([Drawing.ColorTranslator]::FromHtml($color),3);$g.DrawPath($pen,$p);$pen.Dispose();$p.Dispose()
    Line $g ($w*.5) ($h*.5) ($w*.5) 12 $color 3;Dot $g 10 ($h-10) 7 $color;Dot $g ($w-10) 10 7 $color;Dot $g ($w*.5) 12 7 $color
  } else {
    $s=[Math]::Min($w/128,$h/112)*1.8
    $g.TranslateTransform(-18,-12);$g.ScaleTransform([single]$s,[single]$s)
    $b=Brush $color;for($i=0;$i -lt 3;$i++){$g.FillPath($b,$parts[$i].Path)};$b.Dispose()
  };$g.Restore($st)
}
function Desktop($g,$x,$y,$w,$h) {
  $dark=$num -eq '01';$bg=$white;$fg=$ink;$sub='#F1F3F7';$border='#DDE1E8';$secondary=$muted
  if($dark){$bg='#191C23';$fg=$white;$sub='#222630';$border='#333946';$secondary='#A3ADBE'}
  Rect $g $x $y $w $h $bg;Rect $g $x $y $w 38 $sub
  Dot $g ($x+18) ($y+19) 4 $secondary;Dot $g ($x+32) ($y+19) 4 $secondary;Dot $g ($x+46) ($y+19) 4 $secondary
  Text $g 'gitcrab / desktop' ($x+80) ($y+10) 14 $fg 'Consolas'
  Rect $g $x ($y+38) 170 ($h-38) $sub
  Text $g 'WORKSPACE' ($x+18) ($y+61) 10 $secondary
  Text $g 'gitcrab' ($x+18) ($y+88) 18 $fg 'Segoe UI' ([Drawing.FontStyle]::Bold)
  Rect $g ($x+12) ($y+130) 146 32 $blue;Text $g 'History' ($x+25) ($y+137) 14 $ink
  Text $g 'Changes   3' ($x+25) ($y+177) 14 $fg;Text $g 'BRANCHES' ($x+18) ($y+226) 10 $secondary
  Text $g 'main' ($x+25) ($y+254) 14 $fg 'Consolas';Text $g 'feature / logo' ($x+25) ($y+285) 12 $secondary 'Consolas'
  Text $g 'History' ($x+192) ($y+61) 23 $fg 'Segoe UI' ([Drawing.FontStyle]::Bold)
  Text $g 'main' ($x+$w-172) ($y+64) 12 $secondary 'Consolas'
  Rect $g ($x+$w-96) ($y+54) 76 30 $blue;Text $g 'Fetch' ($x+$w-75) ($y+61) 13 $ink
  Line $g ($x+184) ($y+105) ($x+$w-20) ($y+105) $border
  $commits=@('Refine the desktop experience','Merge feature / logo','Add keyboard navigation','Polish repository history','Create workspace')
  $hashes=@('a73d2e1','c18f0b4','d42a7c8','e91b3f2','f20c5a9')
  for($i=0;$i -lt 5;$i++) {
    $cy=$y+135+$i*42
    if($i -eq 0){Rect $g ($x+184) ($cy-10) ($w-204) 37 $sub}
    if($i -lt 4){Line $g ($x+208) $cy ($x+208) ($cy+42) $blue 2}
    Dot $g ($x+208) $cy 4 $blue
    Text $g $commits[$i] ($x+245) ($cy-9) 13 $fg
    Text $g $hashes[$i] ($x+$w-90) ($cy-7) 11 $secondary 'Consolas'
  }
  Line $g ($x+208) ($y+177) ($x+226) ($y+198) $blue 2;Line $g ($x+226) ($y+198) ($x+226) ($y+218) $blue 2;Dot $g ($x+226) ($y+218) 4 $blue
}

$ids=@('01-forkshell','02-branchline','03-splitclaw')
foreach($identity in $ids) {
  Set-Profile $identity
  $dest=Join-Path $root $identity;[void][IO.Directory]::CreateDirectory($dest)
  foreach($variant in @('color','dark','mono','white')) {
    $main=$ink;$accent=$blue;if($variant -eq 'dark'){$main=$white};if($variant -eq 'mono'){$accent=$ink};if($variant -eq 'white'){$main=$white;$accent=$white}
    SaveSvg (Join-Path $dest "symbol-$variant.svg") 160 160 "<g transform='translate(16 27)'>$(SymbolSvg $main $accent)</g>"
    Render (Join-Path $dest "symbol-$variant.png") 1280 1280 {param($g) Symbol $g 128 216 8 $main $accent}
    $w=[Math]::Ceiling($lockW+64)
    SaveSvg (Join-Path $dest "lockup-$variant.svg") $w 176 "<g transform='translate(32 32)'>$(LockSvg $main $accent)</g>"
    Render (Join-Path $dest "lockup-$variant.png") ($w*3) 528 {param($g) Lockup $g 96 96 3 $main $accent}
  }
  foreach($size in @(16,24,32,64,128,256,512,1024)) { Render (Join-Path $dest "app-icon-$size.png") $size $size {param($g) App $g 0 0 $size} }
  SaveSvg (Join-Path $dest 'app-icon.svg') 1024 1024 "<rect width='1024' height='1024' rx='215.04' fill='$ink'/><g transform='translate(118.784 186.368) scale(6.144)'>$(SymbolSvg $white $blue)</g>"
  foreach($size in @(16,24,32)) {
    $s=$size/128
    foreach($variant in @('color','mono','white')) {
      $main=$ink;$accent=$blue;if($variant -eq 'mono'){$accent=$ink};if($variant -eq 'white'){$main=$white;$accent=$white}
      Render (Join-Path $dest "toolbar-$variant-$size.png") $size $size {param($g) Symbol $g 0 ($size/2-53*$s) $s $main $accent}
    }
  }
  $tokens=[ordered]@{identity=$name;colors=[ordered]@{ink=$ink;charcoal=$charcoal;accent=$blue;white=$white;canvas=$canvas;border=$rule;muted=$muted};type=[ordered]@{brandFamily=$family;brandWeight=$weight.ToString();uiFamily='Segoe UI';codeFamily='Consolas'};voice=$tone;headline=$theme;geometry=[ordered]@{symbolWidth=128;clearSpace=16;uiSpacingUnit=8}}
  [IO.File]::WriteAllText((Join-Path $dest 'tokens.json'),($tokens|ConvertTo-Json -Depth 6))
  $pattern=''
  if($num -eq '01') {
    for($i=-2;$i -lt 8;$i++){$off=$i*160;$pattern+="<path d='M$off 400 L$($off+400) 0' stroke='$blue' stroke-width='36'/>"}
  } elseif($num -eq '02') {
    $pattern="<path d='M40 350 C40 60 760 340 760 50 M400 200 V50' stroke='$blue' stroke-width='6' fill='none'/><g fill='$blue'><circle cx='40' cy='350' r='14'/><circle cx='760' cy='50' r='14'/><circle cx='400' cy='50' r='14'/></g>"
  } else {
    $pattern="<g transform='translate(20 -30) scale(7)'>"
    for($i=0;$i -lt 3;$i++){$pattern+="<path fill='$blue' d='$(PathData $parts[$i].Path)'/>"};$pattern+='</g>'
  }
  SaveSvg (Join-Path $dest 'graphic-language.svg') 800 400 $pattern
  Render (Join-Path $dest 'graphic-language.png') 1600 800 {param($g) Motif $g 0 0 1600 800 $blue}
  Render (Join-Path $dest 'product-application.png') 1840 772 {param($g) $g.ScaleTransform(2,2);Desktop $g 0 0 920 386}
  Render (Join-Path $dest 'launch-card.png') 1200 900 {
    param($g)
    $g.Clear([Drawing.ColorTranslator]::FromHtml($ink))
    Text $g 'GITCRAB / A MODERN DESKTOP GIT CLIENT' 72 62 17 '#C7CBD4' 'Consolas'
    Head $g $headline[0] 72 170 96 $white
    Head $g $headline[1] 72 294 96 $white
    Motif $g 650 476 478 350 $blue
    Lockup $g 72 744 (380/$lockW) $white $blue
  }
  Render (Join-Path $dest 'identity-board.png') 1600 1560 {
    param($g)
    $g.Clear([Drawing.Color]::White)
    Text $g 'GITCRAB / THREE DIRECTIONS' 64 36 15 $muted 'Consolas'
    Text $g "$num / $($name.ToUpper())" 1280 36 15 $ink 'Consolas'
    if($num -eq '01') {
      Rect $g 64 92 1472 370 $ink;Lockup $g 112 130 (790/$lockW) $white $blue
      Text $g $tone 116 385 22 '#C7CBD4';Motif $g 1050 92 486 370 $blue
    } elseif($num -eq '02') {
      Rect $g 64 92 1472 370 $canvas;Lockup $g 120 143 (830/$lockW)
      Text $g $tone 124 385 22 $muted;Motif $g 1110 142 345 245 $blue
    } else {
      Rect $g 64 92 1472 370 $canvas
      for($x=64;$x -le 1536;$x+=92){Line $g $x 92 $x 462 '#E5E7EB'}
      for($y=92;$y -le 462;$y+=92){Line $g 64 $y 1536 $y '#E5E7EB'}
      Lockup $g 112 143 (790/$lockW);Text $g $tone 116 385 22 $muted;Motif $g 1170 150 310 260 $blue
    }
    Text $g '01 / LOGO SYSTEM' 64 496 14 $muted 'Consolas'
    $sc=390/$lockW
    Rect $g 64 536 474 164 $canvas;Lockup $g 102 569 $sc
    Rect $g 563 536 474 164 $charcoal;Lockup $g 601 569 $sc $white $blue
    Rect $g 1062 536 474 164 $canvas;Lockup $g 1100 569 $sc $ink $ink
    Text $g 'LIGHT' 80 676 11 $muted 'Consolas';Text $g 'DARK' 579 676 11 '#C4CAD6' 'Consolas';Text $g 'MONOCHROME' 1078 676 11 $muted 'Consolas'
    Text $g '02 / TYPE & COLOR' 64 734 14 $muted 'Consolas'
    Head $g 'Aa Bb 0123' 64 781 60
    Text $g $fontLabel 64 863 16 $muted
    Text $g 'UI: Segoe UI / Code: Consolas' 64 890 14 $muted
    $colors=@($ink,$charcoal,$blue,$canvas)
    for($i=0;$i -lt 4;$i++){$sx=780+$i*192;Rect $g $sx 782 180 78 $colors[$i];Text $g $colors[$i] $sx 878 15 $ink 'Consolas'}
    Text $g '03 / IN THE PRODUCT' 64 956 14 $muted 'Consolas'
    Desktop $g 64 1000 920 386
    Rect $g 1016 1000 520 386 $ink
    Head $g $headline[0] 1048 1040 45 $white
    Head $g $headline[1] 1048 1098 45 $white
    Motif $g 1260 1183 244 171 $blue
    Lockup $g 1048 1302 (180/$lockW) $white $blue
    Text $g 'DESKTOP APP / TOOLBAR AT ACTUAL SIZE' 64 1417 12 $muted 'Consolas'
    App $g 64 1450 64;App $g 152 1482 32
    Symbol $g 260 1474 (.25);Symbol $g 330 1478 (.1875);Symbol $g 396 1482 (.125)
    Text $g '64' 83 1520 11 $muted 'Consolas';Text $g '32' 158 1520 11 $muted 'Consolas'
    Text $g '32' 267 1520 11 $muted 'Consolas';Text $g '24' 333 1520 11 $muted 'Consolas';Text $g '16' 397 1520 11 $muted 'Consolas'
    Text $g 'Flat color. Transparent masters. Outlined wordmarks.' 975 1504 16 $muted
  }
  Write-Output "Built $identity"
}

Render (Join-Path $root 'comparison.png') 1800 1160 {
  param($g)
  $g.Clear([Drawing.Color]::White)
  Text $g 'GITCRAB / THREE VISUAL IDENTITIES' 64 38 17 '#626976' 'Consolas'
  Text $g 'One product. Three distinct characters.' 64 80 42 $ink 'Segoe UI' ([Drawing.FontStyle]::Bold)
  for($j=0;$j -lt 3;$j++) {
    Set-Profile $ids[$j];$x=64+$j*568
    Rect $g $x 178 536 510 $canvas
    Text $g "$num / $($name.ToUpper())" ($x+24) 200 14 $muted 'Consolas'
    Symbol $g ($x+140) 255 2
    $wp=WordPath $family $weight 'GitCrab' 64;$b=Brush $ink;$st=$g.Save();$g.TranslateTransform([single]($x+268-$wp.GetBounds().Width/2),[single]505);$g.FillPath($b,$wp);$g.Restore($st);$b.Dispose();$wp.Dispose()
    Text $g $tone ($x+24) 615 20 $ink
    $cs=@($ink,$charcoal,$blue,$canvas)
    for($k=0;$k -lt 4;$k++){Rect $g ($x+$k*134) 708 134 42 $cs[$k]}
    Text $g $fontLabel $x 776 17 $muted
    App $g $x 832 96;Motif $g ($x+144) 832 392 96 $blue
    Text $g $headline[0] $x 969 29 $ink $family $weight
    Text $g $headline[1] $x 1007 29 $ink $family $weight
  }
  Line $g 64 1090 1736 1090 '#DDE1E8'
  Text $g 'FULL SYSTEMS: LOGOS / TYPOGRAPHY / PALETTE / GRAPHIC LANGUAGE / PRODUCT / ICONS' 64 1113 15 '#626976' 'Consolas'
}
Write-Output 'Built comparison board'
