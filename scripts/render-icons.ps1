$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$taskAssets=Join-Path (Split-Path -Parent $PSScriptRoot) 'assets'
function Render-PortalIcon([int]$size,[bool]$tray,[bool]$online) {
 $scale=4; $bitmap=[Drawing.Bitmap]::new($size*$scale,$size*$scale)
 $g=[Drawing.Graphics]::FromImage($bitmap);$g.SmoothingMode='AntiAlias';$g.Clear([Drawing.Color]::Transparent)
 $g.ScaleTransform($size*$scale/64.0,$size*$scale/64.0)
 if(-not $tray){
  $bg=[Drawing.Drawing2D.GraphicsPath]::new();foreach($a in @(@(4,4,180),@(32,4,270),@(32,32,0),@(4,32,90))){$bg.AddArc($a[0],$a[1],28,28,$a[2],90)};$bg.CloseFigure()
  $brush=[Drawing.Drawing2D.LinearGradientBrush]::new([Drawing.Point]::new(4,4),[Drawing.Point]::new(60,60),[Drawing.Color]::FromArgb(37,99,235),[Drawing.Color]::FromArgb(8,145,178))
  $g.FillPath($brush,$bg);$brush.Dispose();$bg.Dispose()
 }
 $color=if(-not $tray){[Drawing.Color]::White}elseif($online){[Drawing.Color]::FromArgb(37,99,235)}else{[Drawing.Color]::FromArgb(110,119,133)}
 $pen=[Drawing.Pen]::new($color,4.5);$pen.StartCap='Round';$pen.EndCap='Round';$pen.LineJoin='Round'
 $door=[Drawing.Drawing2D.GraphicsPath]::new();$door.AddLine(23,47,23,25);$door.AddBezier(23,25,23,9,47,9,47,25);$door.AddLine(47,25,47,47);$g.DrawPath($pen,$door)
 $g.DrawLine($pen,14,35,38,35);$arrow=[Drawing.Drawing2D.GraphicsPath]::new();$arrow.AddLine(31,28,38,35);$arrow.AddLine(38,35,31,42);$g.DrawPath($pen,$arrow)
 $g.Dispose();$pen.Dispose();$door.Dispose();$arrow.Dispose()
 $out=[Drawing.Bitmap]::new($size,$size);$g=[Drawing.Graphics]::FromImage($out);$g.InterpolationMode='HighQualityBicubic';$g.DrawImage($bitmap,0,0,$size,$size);$g.Dispose();$bitmap.Dispose();return $out
}
$taskFrames=@();foreach($size in @(16,24,32,48,64,128,256)){
 $b=Render-PortalIcon $size $false $true;$stream=[IO.MemoryStream]::new();$b.Save($stream,[Drawing.Imaging.ImageFormat]::Png);$taskFrames+=,@{Size=$size;Bytes=$stream.ToArray()};if($size -eq 256){$b.Save((Join-Path $taskAssets 'portal.png'))};$stream.Dispose();$b.Dispose()
}
$taskIco=[IO.File]::Create((Join-Path $taskAssets 'portal.ico'));$w=[IO.BinaryWriter]::new($taskIco);$w.Write([uint16]0);$w.Write([uint16]1);$w.Write([uint16]$taskFrames.Count);$offset=6+16*$taskFrames.Count
foreach($f in $taskFrames){$n=if($f.Size -eq 256){0}else{$f.Size};$w.Write([byte]$n);$w.Write([byte]$n);$w.Write([uint16]0);$w.Write([uint16]1);$w.Write([uint16]32);$w.Write([uint32]$f.Bytes.Length);$w.Write([uint32]$offset);$offset+=$f.Bytes.Length}
foreach($f in $taskFrames){$w.Write([byte[]]$f.Bytes)};$w.Dispose()
foreach($active in @($true,$false)){
 $b=Render-PortalIcon 32 $true $active;$bytes=[byte[]]::new(4096);for($y=0;$y -lt 32;$y++){for($x=0;$x -lt 32;$x++){$c=$b.GetPixel($x,$y);$i=($y*32+$x)*4;$bytes[$i]=$c.R;$bytes[$i+1]=$c.G;$bytes[$i+2]=$c.B;$bytes[$i+3]=$c.A}};$b.Dispose();$name=if($active){'tray-online.rgba'}else{'tray-waiting.rgba'};[IO.File]::WriteAllBytes((Join-Path $taskAssets $name),$bytes)
}
$b=Render-PortalIcon 32 $false $true;$bytes=[byte[]]::new(4096);for($y=0;$y -lt 32;$y++){for($x=0;$x -lt 32;$x++){$c=$b.GetPixel($x,$y);$i=($y*32+$x)*4;$bytes[$i]=$c.R;$bytes[$i+1]=$c.G;$bytes[$i+2]=$c.B;$bytes[$i+3]=$c.A}};$b.Dispose();[IO.File]::WriteAllBytes((Join-Path $taskAssets 'app.rgba'),$bytes)

