param([switch]$Publish)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $taskRoot
& node scripts/verify-update.cjs dist/update-manifest.json
if($LASTEXITCODE -ne 0){throw 'Release files did not verify'}
$taskVersion=(Get-Content dist/update-manifest.json -Raw|ConvertFrom-Json).version
$taskFiles=@("dist/PortalLoginer-$taskVersion-Setup.exe","dist/PortalLoginer-$taskVersion-windows-x86_64-update.zip",'dist/update-manifest.json')
foreach($taskFile in $taskFiles){if(-not(Test-Path -LiteralPath $taskFile)){throw "Release file missing: $taskFile"}}
if(-not $Publish){Write-Output "Verified release v$taskVersion ready: Setup, update ZIP and manifest.";return}
if(git status --porcelain){throw 'Commit source changes before publishing'}
$taskCommit=git rev-parse HEAD
& gh release create "v$taskVersion" @taskFiles --repo Small-SaltedFish/Portal_Loginer_Client --target $taskCommit --draft --title "Portal Loginer $taskVersion" --notes-file .github/release-notes.md
if($LASTEXITCODE -ne 0){throw 'Draft release upload failed'}
& gh release edit "v$taskVersion" --repo Small-SaltedFish/Portal_Loginer_Client --draft=false --latest
if($LASTEXITCODE -ne 0){throw 'Files uploaded as draft; publish draft after verification'}
