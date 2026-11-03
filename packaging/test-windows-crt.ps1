$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/check-windows-crt.ps1" -LoadOnly

function Expect-Rejected {
    param([string[]]$Dump)
    try {
        Assert-NoDynamicVCRuntime -Executable 'fixture.exe' -Dump $Dump
    } catch {
        return
    }
    throw 'Expected the Windows CRT dependency gate to reject the fixture'
}

Assert-NoDynamicVCRuntime -Executable 'fixture.exe' -Dump @('    KERNEL32.dll', '    ucrtbase.dll')
Assert-NoDynamicVCRuntime -Executable 'fixture.exe' -Dump @('    KERNEL32.dll', '    msvcp_win.dll')
Expect-Rejected -Dump @('    KERNEL32.dll', '    VCRUNTIME140.dll')
Expect-Rejected -Dump @('    KERNEL32.dll', '    VCRUNTIME140_1.dll')
Expect-Rejected -Dump @('    KERNEL32.dll', '    MSVCP140.dll')
Expect-Rejected -Dump @('    KERNEL32.dll', '    MSVCP140_ATOMIC_WAIT.dll')
Expect-Rejected -Dump @('    KERNEL32.dll', '    VCRUNTIME140D.dll')
Expect-Rejected -Dump @('    KERNEL32.dll', '    CONCRT140.dll')
Expect-Rejected -Dump @('dumpbin did not produce dependency output')
Write-Host 'Windows CRT dependency gate regression cases passed'
