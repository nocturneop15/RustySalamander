# Copies the built program out of Cargo's build folder into a clean folder,
# laid out like the release package: the program, its languages, the
# plugins (each with its languages and changelog), the documents and the
# license files. Nothing of Cargo's (deps, build, *.d, *.dll.a, ...).
#
#   tools\stage.ps1                    target\RustySalamander, to run and test
#   tools\stage.ps1 -Out <dir> -Fresh  an empty folder first: the release
#                                      package (no settings in it)
#
# Without -Fresh, what the program saved in the folder stays (config.toml,
# known_hosts, last-connection, debug logs); when the folder is new, the
# settings saved in target\<profile> are brought over.
param(
    [string]$Out,
    [string]$BuildProfile = "release",
    [switch]$Fresh
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$built = Join-Path $root "target\$BuildProfile"
if (-not $Out) { $Out = Join-Path $root "target\RustySalamander" }
if (-not (Test-Path (Join-Path $built "rustysalamander.exe"))) { throw "Not built: $built\rustysalamander.exe (cargo build --$BuildProfile)" }

$new = -not (Test-Path $Out)
if ($Fresh -and -not $new) { Remove-Item $Out -Recurse -Force; $new = $true }
New-Item -ItemType Directory -Force $Out | Out-Null

function Put([string]$from, [string]$to) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $to) | Out-Null
    Copy-Item $from $to -Force
}

# the program and its documents
Put "$built\rustysalamander.exe" "$Out\rustysalamander.exe"
Get-ChildItem "$root\crates\sal-app\lang\*.toml" | ForEach-Object { Put $_.FullName "$Out\lang\$($_.Name)" }
foreach ($doc in "README.md", "CHANGELOG.md", "THIRD-PARTY-NOTICES.md") { Put "$root\$doc" "$Out\$doc" }
Put "$root\LICENSE" "$Out\LICENSE.txt"
Put "$root\docs\USER-MANUAL.md" "$Out\USER-MANUAL.md"
Put "$root\crates\sal-ui\fonts\materialdesignicons-LICENSE.txt" "$Out\MaterialDesignIcons-LICENSE.txt"

# the plugins the build installed (target\<profile>\plugins\<id>), with the
# changelog from their source folder (plugins\<source>)
$sources = @{ "7zip" = "sevenzip" }
foreach ($dir in Get-ChildItem "$built\plugins" -Directory) {
    $id = $dir.Name
    $dst = "$Out\plugins\$id"
    # the plugin's own files: its library, 7z.dll and its licenses
    foreach ($f in Get-ChildItem $dir.FullName -File | Where-Object { $_.Extension -in ".spl", ".dll", ".txt" }) { Put $f.FullName "$dst\$($f.Name)" }
    Get-ChildItem "$($dir.FullName)\lang\*.toml" -ErrorAction SilentlyContinue | ForEach-Object { Put $_.FullName "$dst\lang\$($_.Name)" }
    $src = if ($sources.ContainsKey($id)) { $sources[$id] } else { $id }
    if (Test-Path "$root\plugins\$src\CHANGELOG.md") { Put "$root\plugins\$src\CHANGELOG.md" "$dst\CHANGELOG.md" }
}

# a new test folder starts with the settings of the build folder
if ($new -and -not $Fresh) {
    if (Test-Path "$built\config.toml") { Put "$built\config.toml" "$Out\config.toml" }
    foreach ($dir in Get-ChildItem "$built\plugins" -Directory) {
        foreach ($f in "known_hosts", "last-connection") {
            if (Test-Path "$($dir.FullName)\$f") { Put "$($dir.FullName)\$f" "$Out\plugins\$($dir.Name)\$f" }
        }
    }
}

$count = (Get-ChildItem $Out -Recurse -File).Count
Write-Output "$Out ($count files)"
