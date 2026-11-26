# Dot-source before building: . .\env.ps1
# Puts cargo and the MinGW-w64 toolchain (needed by the x86_64-pc-windows-gnu target) on PATH.
$mingw = (Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs*\mingw64\bin" -Directory -ErrorAction SilentlyContinue | Select-Object -First 1).FullName
$env:Path = "$env:USERPROFILE\.cargo\bin;$mingw;$env:Path"

# Temporary files of builds and tests (gcc's cc* files, the tests' temporary
# folders) go to target\tmp, not %TEMP%: what is left there (a test that
# failed, a DLL a test still had loaded) is cleared after a day, and with
# cargo clean.
$tmp = Join-Path $PSScriptRoot 'target\tmp'
New-Item -ItemType Directory -Force $tmp | Out-Null
Get-ChildItem $tmp -Force -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-1) } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
$env:TMP = $tmp
$env:TEMP = $tmp
