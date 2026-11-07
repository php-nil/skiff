$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
$RepoMarker = Join-Path $RepoRoot 'Cargo.toml'
$cwd = (Get-Location).Path
$inRepo = (Test-Path $RepoMarker) -and $cwd.StartsWith($RepoRoot, [StringComparison]::OrdinalIgnoreCase)

function Resolve-RealGit {
    if ($env:SKIFF_REAL_GIT -and (Test-Path $env:SKIFF_REAL_GIT)) { return $env:SKIFF_REAL_GIT }
    $wrapperDir = Join-Path $RepoRoot 'tools\bin'
    foreach ($p in ($env:PATH -split ';')) {
        if (-not $p) { continue }
        if ($p.TrimEnd('\') -eq $wrapperDir.TrimEnd('\')) { continue }
        $exe = Join-Path $p 'git.exe'
        if (Test-Path $exe) { return $exe }
    }
    $fallback = Join-Path $env:ProgramFiles 'Git\cmd\git.exe'
    if (Test-Path $fallback) { return $fallback }
    throw '找不到真实的 git.exe（PATH 中除包装入口外没有 git.exe）'
}

if (-not $inRepo) {
    $real = Resolve-RealGit
    & $real @args
    exit $LASTEXITCODE
}

$gateParams = @{ Task = 'git'; GitArgs = $args }
& (Join-Path $PSScriptRoot 'gate.ps1') @gateParams
exit $LASTEXITCODE
