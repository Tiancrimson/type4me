[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet("dev", "build")]
    [string]$Task,

    [switch]$DebugBuild,
    [switch]$NoBundle
)

$ErrorActionPreference = "Stop"

$vsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path -LiteralPath $vsWhere)) {
    throw "vswhere.exe was not found. Install Visual Studio Build Tools with the C++ workload."
}

$vsInstallPath = & $vsWhere `
    -latest `
    -products * `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath

if (-not $vsInstallPath) {
    throw "MSVC was not found. Install the 'Desktop development with C++' workload."
}

$devShell = Join-Path $vsInstallPath "Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Import-Module $devShell
Enter-VsDevShell `
    -VsInstallPath $vsInstallPath `
    -SkipAutomaticLocation `
    -DevCmdArguments "-arch=x64 -host_arch=x64" | Out-Null

$cargoBin = Join-Path $HOME ".cargo\bin"
if (($env:Path -split ";") -notcontains $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

if ($Task -eq "dev") {
    npm run tauri -- dev
} else {
    $arguments = @("run", "tauri", "--", "build")
    if ($DebugBuild) {
        $arguments += "--debug"
    }
    if ($NoBundle) {
        $arguments += "--no-bundle"
    }
    npm @arguments
}

exit $LASTEXITCODE
