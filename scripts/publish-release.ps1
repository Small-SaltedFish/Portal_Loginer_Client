param([switch]$Publish)
$ErrorActionPreference='Stop'
$taskRoot=Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $taskRoot
& node scripts/sign-update.cjs verify dist/update-manifest.json dist/update-manifest.sig
if($LASTEXITCODE -ne 0){throw 'Release files did not verify'}
$taskVersion=(Get-Content dist/update-manifest.json -Raw|ConvertFrom-Json).version
$taskFiles=@("dist/PortalLoginer-$taskVersion-Setup.exe","dist/PortalLoginer-$taskVersion-Cleanup.exe","dist/PortalLoginer-$taskVersion-windows-x86_64-update.zip","dist/PortalLoginer-$taskVersion-Setup.exe.sha256","dist/PortalLoginer-$taskVersion-Cleanup.exe.sha256",'dist/update-manifest.json','dist/update-manifest.sig')
foreach($taskFile in $taskFiles){if(-not(Test-Path -LiteralPath $taskFile)){throw "Release file missing: $taskFile"}}
if(-not $Publish){Write-Output "Verified release v$taskVersion is ready. Use -Publish to upload these public artifacts only.";return}
if(git status --porcelain){throw 'Commit source changes before publishing'}
$taskCommit=git rev-parse HEAD
& gh release create "v$taskVersion" @taskFiles --repo Small-SaltedFish/Portal_Loginer_Client --target $taskCommit --latest --title "Portal Loginer $taskVersion" --notes-file .github/release-notes.md
if($LASTEXITCODE -ne 0){throw 'Release publication failed'}
