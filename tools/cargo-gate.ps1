# PATH 包装入口的真实实现：把 cargo 参数原样送进闸门，非仓库目录直接透传。
# 本文件不带 param 块，全部参数落在自动变量 $args，避免 PowerShell 把 cargo 的
# `-D warnings`、`--` 之类 token 当成自身参数解析失败。
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$RepoMarker = Join-Path $RepoRoot 'Cargo.toml'
$cwd = (Get-Location).Path

# 非本仓库目录：不占用闸门，直接调用真实 cargo（避免污染整机其它项目的 cargo 用法）
$inRepo = (Test-Path $RepoMarker) -and $cwd.StartsWith($RepoRoot, [StringComparison]::OrdinalIgnoreCase)
if (-not $inRepo) {
    $real = $env:SKIFF_REAL_CARGO
    if (-not $real) {
        $real = (Get-Command cargo.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    }
    & $real @args
    exit $LASTEXITCODE
}

$gateParams = @{ Task = 'exec'; ExecArgs = $args }
& (Join-Path $PSScriptRoot 'gate.ps1') @gateParams
exit $LASTEXITCODE
