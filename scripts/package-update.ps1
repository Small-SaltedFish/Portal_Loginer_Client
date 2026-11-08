param([string]$Version)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
if(-not $Version){$Version=[regex]::Match((Get-Content (Join-Path $taskRoot 'Cargo.toml') -Raw),'(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value}
if($Version -notmatch '^\d+\.\d+\.\d+$'){throw 'Stable version required'}
$taskStaging=Join-Path $taskRoot "target/update-payload-$Version"
New-Item -ItemType Directory $taskStaging -Force | Out-Null
$taskExtract=Join-Path $taskRoot "target/update-uninstaller-$Version"
& 7z.exe x -y "-o$taskExtract" (Join-Path $taskRoot "dist/PortalLoginer-$Version-Setup.exe") 'Uninstall.exe' | Out-Null
if($LASTEXITCODE -ne 0){throw 'Cannot extract standalone uninstaller'}
foreach($taskName in @('portal-service.exe','portal-tray.exe','portal-settings.exe')){Copy-Item (Join-Path $taskRoot "target/release/$taskName") (Join-Path $taskStaging $taskName) -Force}
Copy-Item (Join-Path $taskExtract 'Uninstall.exe') (Join-Path $taskStaging 'Uninstall.exe') -Force
foreach($taskName in @('LICENSE','NOTICE','THIRD-PARTY-NOTICES.txt')){Copy-Item (Join-Path $taskRoot $taskName) (Join-Path $taskStaging $taskName) -Force}
$taskName="PortalLoginer-$Version-windows-x86_64-update.zip"
$taskPayload=Join-Path $taskRoot "dist/$taskName"
$taskFiles=@('portal-service.exe','portal-tray.exe','portal-settings.exe','Uninstall.exe','LICENSE','NOTICE','THIRD-PARTY-NOTICES.txt') | ForEach-Object{Join-Path $taskStaging $_}
Compress-Archive -LiteralPath $taskFiles -DestinationPath $taskPayload -Force
$taskManifest=[ordered]@{schema=1;repository='Small-SaltedFish/Portal_Loginer_Client';version=$Version;assets=@([ordered]@{platform='windows';architecture='x86_64';name=$taskName;url="https://github.com/Small-SaltedFish/Portal_Loginer_Client/releases/download/v$Version/$taskName";size=(Get-Item $taskPayload).Length;sha256=(Get-FileHash $taskPayload -Algorithm SHA256).Hash.ToLowerInvariant()})}
$taskManifestPath=Join-Path $taskRoot 'dist/update-manifest.json'
[IO.File]::WriteAllText($taskManifestPath,($taskManifest|ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
$taskHash=(Get-FileHash $taskPayload -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$taskPayload.sha256", "$taskHash  $taskName`n", [Text.UTF8Encoding]::new($false))
Get-Item $taskPayload,$taskManifestPath | Select-Object Name,Length
