param([string]$Resources, [string]$World='OwnedNpcApiProof')
$ErrorActionPreference='Stop'
$packageDir=$PSScriptRoot
$game=Join-Path $packageDir 'horizon-client.exe'
$ctl=Join-Path $packageDir 'horizon-ctl.exe'
if(-not (Test-Path $game)){throw 'The compiled horizon-client.exe is missing.'}
if(-not $Resources){
    $versionRoot=Join-Path $env:APPDATA 'horizon-launcher/versions'
    $settingsPath=Join-Path $env:APPDATA 'horizon-launcher/settings.json'
    if(Test-Path $settingsPath){
        $selected=(Get-Content $settingsPath -Raw|ConvertFrom-Json).selected
        $candidate=Join-Path $versionRoot "$selected/.local/horizon-client/26.3"
        if(Test-Path (Join-Path $candidate 'horizon-resources.json')){$Resources=$candidate}
    }
    if(-not $Resources){
        foreach($installed in (Get-ChildItem $versionRoot -Directory|Sort-Object LastWriteTime -Descending)){
            $candidate=Join-Path $installed.FullName '.local/horizon-client/26.3'
            if(Test-Path (Join-Path $candidate 'horizon-resources.json')){$Resources=$candidate;break}
        }
    }
}
if(-not $Resources -or -not (Test-Path (Join-Path $Resources 'horizon-resources.json'))){
    throw 'Prepared Minecraft 26.3 resources not found. Prepare Horizon in its launcher, or pass -Resources PATH.'
}
$dataDir=Join-Path $packageDir 'game-data'
New-Item -ItemType Directory -Force $dataDir|Out-Null
$env:HORIZON_HOME=$dataDir
$socket=Join-Path $dataDir 'control.json'
& $ctl --socket $socket --timeout 2 status 2>$null|Out-Null
if($LASTEXITCODE -eq 0){Write-Host 'This fork application is already running.';exit 0}
& $ctl --socket $socket launch --game $game --world $World --creative --resources $Resources --render-distance 6 --no-pause --wait
if($LASTEXITCODE){throw 'Could not start the fork. See game-data/control.log.'}
Write-Host 'Started the Civilization fork. Its worlds are stored only in game-data.'
