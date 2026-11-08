param([int]$InterfaceIndex = 0)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
$taskFolder = Join-Path $taskRoot 'target/portal-diagnostics'
New-Item -ItemType Directory -Force $taskFolder | Out-Null
$taskLog = Join-Path $taskFolder ("network-{0}.jsonl" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
function Record($value) {
    $value.time = [DateTimeOffset]::UtcNow.ToString('o')
    $line = $value | ConvertTo-Json -Compress -Depth 8
    Add-Content -LiteralPath $taskLog -Value $line -Encoding utf8
    Write-Output $line
}
$taskNics = @([Net.NetworkInformation.NetworkInterface]::GetAllNetworkInterfaces() | Where-Object {
    $_.OperationalStatus -eq 'Up' -and ($InterfaceIndex -ne 0 -or $_.NetworkInterfaceType -eq 'Wireless80211')
} | Where-Object {try { $InterfaceIndex -eq 0 -or $_.GetIPProperties().GetIPv4Properties().Index -eq $InterfaceIndex } catch { $false }})
if ($taskNics.Count -ne 1) { throw 'Specify exactly one connected Wi-Fi interface; no fallback to another interface.' }
$taskNic = $taskNics[0]
$taskProperties = $taskNic.GetIPProperties()
$taskIndex = $taskProperties.GetIPv4Properties().Index
$taskAddress = @($taskProperties.UnicastAddresses | Where-Object {$_.Address.AddressFamily -eq 'InterNetwork' -and -not $_.Address.ToString().StartsWith('169.254.')})[0].Address
if (-not $taskAddress) { throw 'Selected interface has no usable IPv4 address.' }
Record @{event='interface'; name=$taskNic.Name; index=$taskIndex; source=$taskAddress.ToString(); dns=@($taskProperties.DnsAddresses | ForEach-Object {$_.ToString()}); mode='IP_UNICAST_IF+source_bind'; authentication_requests=$false}

function New-InterfaceSocket([Net.Sockets.SocketType]$type, [Net.Sockets.ProtocolType]$protocol) {
    $socket = [Net.Sockets.Socket]::new([Net.Sockets.AddressFamily]::InterNetwork,$type,$protocol)
    try {
        # https://learn.microsoft.com/en-us/windows/win32/winsock/ipproto-ip-socket-options
        $socket.SetSocketOption([Net.Sockets.SocketOptionLevel]::IP,[Net.Sockets.SocketOptionName]31,[Net.IPAddress]::HostToNetworkOrder([int]$taskIndex))
        $socket.Bind([Net.IPEndPoint]::new($taskAddress,0))
        $socket.ReceiveTimeout=2500; $socket.SendTimeout=2500
        return $socket
    } catch { $socket.Dispose(); throw }
}
function Skip-DnsName([byte[]]$bytes, [int]$offset) {
    for($n=0;$n -lt 128;$n++) {
        if($offset -ge $bytes.Length) {throw 'Truncated DNS name'}
        $length=[int]$bytes[$offset];$offset++
        if($length -eq 0){return $offset}
        if(($length -band 192) -eq 192){return ($offset+1)}
        if($length -gt 63){throw 'Invalid DNS name'}
        $offset += $length
    }
    throw 'DNS name too long'
}
function Resolve-InterfaceA([string]$domain) {
    $servers=@($taskProperties.DnsAddresses | Where-Object {$_.AddressFamily -eq 'InterNetwork'}) + @([Net.IPAddress]::Parse('223.5.5.5'),[Net.IPAddress]::Parse('119.29.29.29'))
    foreach($server in $servers | Select-Object -Unique) {
        $socket=New-InterfaceSocket Dgram Udp
        $watch=[Diagnostics.Stopwatch]::StartNew()
        try {
            $id=Get-Random -Minimum 1 -Maximum 65535
            $query=[Collections.Generic.List[byte]]::new()
            $query.AddRange([byte[]]@(($id -shr 8),($id -band 255),1,0,0,1,0,0,0,0,0,0))
            foreach($part in $domain.Split('.')){$query.Add([byte]$part.Length);$query.AddRange([Text.Encoding]::ASCII.GetBytes($part))}
            $query.AddRange([byte[]]@(0,0,1,0,1))
            $socket.Connect([Net.IPEndPoint]::new($server,53))
            [void]$socket.Send($query.ToArray())
            $buffer=[byte[]]::new(4096);$length=$socket.Receive($buffer)
            if($length -lt 12 -or $buffer[0] -ne ($id -shr 8) -or $buffer[1] -ne ($id -band 255)){throw 'Invalid DNS response'}
            $rcode=$buffer[3] -band 15
            $questions=([int]$buffer[4] -shl 8)+$buffer[5];$answers=([int]$buffer[6] -shl 8)+$buffer[7];$offset=12
            for($n=0;$n -lt $questions;$n++){$offset=(Skip-DnsName $buffer $offset)+4}
            $addresses=@()
            for($n=0;$n -lt $answers;$n++) {
                $offset=Skip-DnsName $buffer $offset
                if($offset+10 -gt $length){throw 'Truncated DNS answer'}
                $kind=([int]$buffer[$offset] -shl 8)+$buffer[$offset+1];$size=([int]$buffer[$offset+8] -shl 8)+$buffer[$offset+9];$offset+=10
                if($offset+$size -gt $length){throw 'Truncated DNS data'}
                if($kind -eq 1 -and $size -eq 4){$addresses+=([Net.IPAddress]::new([byte[]]$buffer[$offset..($offset+3)])).ToString()}
                $offset+=$size
            }
            Record @{event='dns';domain=$domain;server=$server.ToString();answers=$addresses;rcode=$rcode;elapsed_ms=$watch.ElapsedMilliseconds;source=$socket.LocalEndPoint.ToString()}
            $usable=@($addresses | Where-Object {$_ -notmatch '^(198\.(18|19)\.|127\.|0\.|169\.254\.)'})
            if($usable.Count){return $usable[0]}
        } catch {Record @{event='dns_error';domain=$domain;server=$server.ToString();elapsed_ms=$watch.ElapsedMilliseconds;error=$_.Exception.GetBaseException().Message}}
        finally {$socket.Dispose()}
    }
    return $null
}
function Invoke-InterfaceHttp([string]$domain,[string]$address,[string]$path,[string]$method='GET') {
    $socket=New-InterfaceSocket Stream Tcp
    $watch=[Diagnostics.Stopwatch]::StartNew()
    try {
        $pending=$socket.ConnectAsync([Net.IPAddress]::Parse($address),80)
        if(-not $pending.Wait(4000)){throw 'TCP connect timeout'}
        $request="$method $path HTTP/1.0`r`nHost: $domain`r`nUser-Agent: PortalLoginer-Diagnostics`r`nConnection: close`r`nContent-Length: 0`r`n`r`n"
        $bytes=[Text.Encoding]::ASCII.GetBytes($request);$sent=0
        while($sent -lt $bytes.Length){$sent+=$socket.Send($bytes,$sent,$bytes.Length-$sent,[Net.Sockets.SocketFlags]::None)}
        $content=[IO.MemoryStream]::new();$buffer=[byte[]]::new(4096)
        while($content.Length -lt 65536 -and $watch.ElapsedMilliseconds -lt 8000) {
            $size=$socket.Receive($buffer);if($size -eq 0){break};$content.Write($buffer,0,$size)
            if($method -eq 'GET' -and [Text.Encoding]::ASCII.GetString($content.ToArray()).Contains("`r`n`r`n")){break}
        }
        $text=[Text.Encoding]::UTF8.GetString($content.ToArray());$content.Dispose()
        $headers,$body=$text -split "`r`n`r`n",2
        $status=if($headers -match '^HTTP/\S+\s+(\d+)'){[int]$Matches[1]}else{$null}
        $result=@{event='http';domain=$domain;path=$path;status=$status;source=$socket.LocalEndPoint.ToString();remote=$socket.RemoteEndPoint.ToString();interface=$taskIndex;elapsed_ms=$watch.ElapsedMilliseconds}
        if($headers -match '(?im)^Location:\s*(.+)$') {
            $location=$Matches[1].Trim();$uri=$null
            if([uri]::TryCreate($location,[UriKind]::Absolute,[ref]$uri)) {
                $result.redirect=$uri.GetLeftPart([UriPartial]::Path)
                $result.parameters=@{}
                foreach($part in $uri.Query.TrimStart('?').Split('&')) {
                    $key,$value=$part -split '=',2
                    $result.parameters[$key]=if($key -in @('bras','clientip','wlanuserip','wlanacname','vlan','paip')){$value}else{'[redacted]'}
                }
            }else{$result.redirect='non-absolute'}
        }
        if($method -eq 'POST' -and $path -eq '/api/ip.php' -and $body) {
            try{$data=$body | ConvertFrom-Json;$result.portal=@{ret=$data.ret;ip=$data.data.ip;logined=$data.data.logined;sessionlogined=$data.data.sessionlogined;matches_source=($data.data.ip -eq $taskAddress.ToString())}}catch{$result.portal='unparseable'}
        }
        Record $result
    }catch{Record @{event='http_error';domain=$domain;path=$path;interface=$taskIndex;elapsed_ms=$watch.ElapsedMilliseconds;error=$_.Exception.GetBaseException().Message}}
    finally{$socket.Dispose()}
}
# No login, ACK or logout requests, and no credentials are read.
foreach($domain in @('connect.rom.miui.com','connectivitycheck.platform.hicloud.com','wifi.vivo.com.cn')) {
    $answer=@(Resolve-InterfaceA $domain)
    foreach($line in $answer){if($line -is [string] -and $line.StartsWith('{')){Write-Output $line}}
    $address=$answer | Where-Object {$_ -is [string] -and $_ -match '^\d+\.\d+\.\d+\.\d+$'} | Select-Object -First 1
    if($address){Invoke-InterfaceHttp $domain $address '/generate_204'}
}
Invoke-InterfaceHttp '192.168.199.200' '192.168.199.200' '/index.html'
Invoke-InterfaceHttp '192.168.199.200' '192.168.199.200' '/api/ip.php' 'POST'
Record @{event='complete';log=$taskLog}
