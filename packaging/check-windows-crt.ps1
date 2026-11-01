param(
    [string]$Stage = 'stage',
    [switch]$LoadOnly
)

$ErrorActionPreference = 'Stop'

function Assert-NoDynamicVCRuntime {
    param([string]$Executable, [string[]]$Dump)

    $imports = @($Dump | ForEach-Object {
        if ($_ -match '^\s*([A-Za-z0-9_.-]+\.dll)\s*$') { $Matches[1] }
    })
    if ($imports.Count -eq 0) {
        throw "No DLL imports found in dumpbin output for ${Executable}; cannot verify dependencies"
    }
    $forbidden = @($imports | Where-Object { $_ -match '^(vcruntime|msvcp|msvcr|vccorlib|concrt)[0-9][a-z0-9_]*\.dll$' })
    if ($forbidden.Count -ne 0) {
        throw "${Executable} requires Visual C++ redistributable DLLs: $($forbidden -join ', ')"
    }
}

if ($LoadOnly) { return }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (!(Test-Path -LiteralPath $vswhere)) { throw "vswhere not found: $vswhere" }
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or !$vs) { throw 'Visual Studio MSVC tools not found by vswhere' }
$dumpbin = Get-ChildItem -Path (Join-Path $vs 'VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe') |
    Sort-Object FullName -Descending | Select-Object -First 1
if (!$dumpbin) { throw "dumpbin.exe not found in $vs" }

foreach ($name in @('solaris-launcher.exe', 'solaris-client.exe', 'solaris-v2.exe')) {
    $exe = Join-Path $Stage $name
    if (!(Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Missing staged executable: $exe" }
    $dump = & $dumpbin.FullName /dependents $exe 2>&1
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed for ${exe}: $dump" }
    Assert-NoDynamicVCRuntime -Executable $exe -Dump $dump
    Write-Host "Verified VC runtime imports: $exe"
}
