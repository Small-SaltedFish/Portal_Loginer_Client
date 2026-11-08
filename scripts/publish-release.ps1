param([switch]$Publish)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $taskRoot
& node scripts/verify-update.cjs dist/update-manifest.json
if($LASTEXITCODE -ne 0){throw 'Release files did not verify'}
$taskManifest=Get-Content dist/update-manifest.json -Raw|ConvertFrom-Json
$taskVersion=$taskManifest.version
$taskSetup="dist/PortalLoginer-$taskVersion-Setup.exe"
$taskPayload="dist/PortalLoginer-$taskVersion-windows-x86_64-update.zip"
if(-not(Test-Path -LiteralPath $taskSetup)){throw 'Installer missing'}
if(-not $Publish){Write-Output "Verified v$taskVersion is ready: installer in Releases; manifest and ZIP in updates/.";return}
if(git status --porcelain){throw 'Commit source changes before publishing'}
$taskSourceCommit=git rev-parse HEAD
New-Item -ItemType Directory updates -Force | Out-Null
Copy-Item -LiteralPath $taskPayload -Destination updates/windows-x86_64.zip -Force
git add -f updates/windows-x86_64.zip
git diff --cached --quiet
if($LASTEXITCODE -eq 1){git commit -m "Publish update payload $taskVersion [skip ci]";if($LASTEXITCODE -ne 0){throw 'Cannot commit update payload'}}
$taskPayloadCommit=git rev-parse HEAD
git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push
if($LASTEXITCODE -ne 0){throw 'Cannot publish repository update payload'}
$taskManifest.assets[0].url="https://raw.githubusercontent.com/Small-SaltedFish/Portal_Loginer_Client/$taskPayloadCommit/updates/windows-x86_64.zip"
[IO.File]::WriteAllText((Join-Path $taskRoot 'dist/update-manifest.json'),($taskManifest|ConvertTo-Json -Depth 5),[Text.UTF8Encoding]::new($false))
& node scripts/verify-update.cjs dist/update-manifest.json
if($LASTEXITCODE -ne 0){throw 'Repository payload verification failed'}
$taskExisting=& gh release view "v$taskVersion" --repo Small-SaltedFish/Portal_Loginer_Client --json assets 2>$null
if($LASTEXITCODE -eq 0){
    $taskRemote=gh api "repos/Small-SaltedFish/Portal_Loginer_Client/releases/tags/v$taskVersion" | ConvertFrom-Json
    $taskAsset=@($taskRemote.assets|Where-Object {$_.name -eq [IO.Path]::GetFileName($taskSetup)})
    $taskDigest='sha256:'+(Get-FileHash $taskSetup -Algorithm SHA256).Hash.ToLowerInvariant()
    if($taskAsset.Count -ne 1 -or $taskAsset[0].digest -ne $taskDigest){throw 'Existing installer does not match; refusing to replace it'}
}else{
    & gh release create "v$taskVersion" $taskSetup --repo Small-SaltedFish/Portal_Loginer_Client --target $taskSourceCommit --latest --title "Portal Loginer $taskVersion" --notes-file .github/release-notes.md
    if($LASTEXITCODE -ne 0){throw 'Installer release publication failed'}
}
Copy-Item -LiteralPath dist/update-manifest.json -Destination updates/manifest.json -Force
git add updates/manifest.json
git diff --cached --quiet
if($LASTEXITCODE -eq 1){git commit -m "Publish update manifest $taskVersion [skip ci]";if($LASTEXITCODE -ne 0){throw 'Cannot commit update manifest'}}
git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push
if($LASTEXITCODE -ne 0){throw 'Installer published; manifest push failed, rerun this script to finish'}
