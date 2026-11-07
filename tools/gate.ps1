<#
单写者编译闸门。本机所有 cargo fmt / clippy / test / build 的唯一入口。

设计要点（2026-11-07 卡死事故复盘落地）：
  1. Windows 命名互斥体 Global\SkiffBuild：全机同一时刻只允许一路编译，
     进程死亡自动释放，不会留下无法判断存活的目录锁。
  2. 租约文件 .workbuddy/locks/build.lease：记录 pid / 会话 / 命令 / 开始时间，
     仅持锁进程可在 finally 中释放；陈旧租约自动回收并写日志。
  3. 拿不到锁立即以退出码 2 结束，不排队、不等待。
  4. 根 workspace 与 plugins/ 共用同一把互斥体，禁止并行。
  5. 提交内存准入：% Committed Bytes In Use > 80% 或 Available < 1536MB 时拒绝启动（退出码 3）。
  6. 运行期间每 5 秒采样一次，写入 .workbuddy/memwatch/YYYYMMDD.csv，保留 7 天。
  7. 本地全量拦截：根 workspace 的 clippy / test / build 不带 -Pkg 时必须显式 -AllowFull（退出码 4）。

退出码：0 成功；2 锁被占用；3 内存准入拒绝；4 需要 -AllowFull；其他为 cargo 自身的退出码。
用法示例：
  pwsh tools/gate.ps1 -Task fmt
  pwsh tools/gate.ps1 -Task test -Pkg skiff-policy
  pwsh tools/gate.ps1 -Task clippy -Pkg skiff-web
  pwsh tools/gate.ps1 -Task test -Plugins
  pwsh tools/gate.ps1 -Task test -AllowFull
  pwsh tools/gate.ps1 -Task build -Pkg skiff -AllowFull
  pwsh tools/gate.ps1 -Task probe -HoldSeconds 6   # 仅用于验证互斥生效，不跑 cargo
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('fmt', 'clippy', 'test', 'build', 'probe')]
    [string]$Task,

    [string]$Pkg,

    [switch]$Plugins,

    [switch]$AllowFull,

    [switch]$DryRun,

    [int]$HoldSeconds = 6
)

$ErrorActionPreference = 'Stop'

$RepoRoot  = Split-Path -Parent $PSScriptRoot
$LockDir   = Join-Path $RepoRoot '.workbuddy\locks'
$MemDir    = Join-Path $RepoRoot '.workbuddy\memwatch'
$LeaseFile = Join-Path $LockDir 'build.lease'
$GateLog   = Join-Path $LockDir 'gate.log'
$StopFile  = Join-Path $LockDir ("sampler.stop.{0}" -f $PID)

$MaxCommitPercent = 80
$MinAvailableMB   = 1536
$RetentionDays    = 7

foreach ($d in @($LockDir, $MemDir)) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
}

function Write-GateLog([string]$line) {
    $ts = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Path $GateLog -Value ("{0} pid={1} {2}" -f $ts, $PID, $line) -Encoding utf8
}

# 清理超过 7 天的采样文件
Get-ChildItem $MemDir -Filter '*.csv' -File -ErrorAction SilentlyContinue |
    Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-$RetentionDays) } |
    Remove-Item -Force -ErrorAction SilentlyContinue

function Get-GateMutex {
    $names = @('Global\SkiffBuild', 'Local\SkiffBuild')
    foreach ($n in $names) {
        try {
            return [System.Threading.Mutex]::new($false, $n)
        } catch {
            # 无 SeCreateGlobalPrivilege 时退回当前会话命名空间
        }
    }
    throw '无法创建编译闸门互斥体'
}

function Get-LeaseOwner {
    if (-not (Test-Path $LeaseFile)) { return 'unknown' }
    try {
        $j = Get-Content $LeaseFile -Raw | ConvertFrom-Json
        return ('pid={0} since={1} task={2} pkg={3} plugins={4}' -f $j.pid, $j.startedAt, $j.task, $j.pkg, $j.plugins)
    } catch {
        return 'unknown(lease-unreadable)'
    }
}

function Get-MemSnapshot {
    $counter = Get-Counter -Counter @(
        '\Memory\Committed Bytes',
        '\Memory\Commit Limit',
        '\Memory\% Committed Bytes In Use',
        '\Memory\Available MBytes',
        '\Paging File(_Total)\% Usage'
    ) -ErrorAction Stop
    $s = $counter.CounterSamples
    return [pscustomobject]@{
        CommittedMB = [math]::Round($s[0].CookedValue / 1MB, 0)
        LimitMB     = [math]::Round($s[1].CookedValue / 1MB, 0)
        CommitPct   = [math]::Round($s[2].CookedValue, 0)
        AvailableMB = [math]::Round($s[3].CookedValue, 0)
        PagefilePct = [math]::Round($s[4].CookedValue, 0)
    }
}

function Start-Sampler([string]$csvPath) {
    if (-not (Test-Path $csvPath)) {
        Set-Content -Path $csvPath -Value 'ts,committedMB,commitLimitMB,commitPct,availableMB,pagefilePct,cargoCount,rustcCount' -Encoding utf8
    }
    if (Test-Path $StopFile) { Remove-Item $StopFile -Force }
    $rs = [runspacefactory]::CreateRunspace()
    $rs.Open()
    $ps = [powershell]::Create()
    $ps.Runspace = $rs
    $null = $ps.AddScript({
        param($csv, $stop)
        while (-not (Test-Path $stop)) {
            try {
                $c = Get-Counter -Counter @(
                    '\Memory\Committed Bytes',
                    '\Memory\Commit Limit',
                    '\Memory\% Committed Bytes In Use',
                    '\Memory\Available MBytes',
                    '\Paging File(_Total)\% Usage'
                ) -ErrorAction Stop
                $s = $c.CounterSamples
                $cargo = @(Get-Process -Name cargo -ErrorAction SilentlyContinue).Count
                $rustc = @(Get-Process -Name rustc -ErrorAction SilentlyContinue).Count
                $row = '{0},{1},{2},{3},{4},{5},{6},{7}' -f `
                    (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'),
                    [math]::Round($s[0].CookedValue / 1MB, 0),
                    [math]::Round($s[1].CookedValue / 1MB, 0),
                    [math]::Round($s[2].CookedValue, 0),
                    [math]::Round($s[3].CookedValue, 0),
                    [math]::Round($s[4].CookedValue, 0),
                    $cargo, $rustc
                Add-Content -Path $csv -Value $row -Encoding utf8
            } catch {
                # 单次采样失败不影响构建
            }
            Start-Sleep -Seconds 5
        }
    }).AddArgument($csvPath).AddArgument($StopFile)
    $handle = $ps.BeginInvoke()
    return [pscustomobject]@{ PowerShell = $ps; Runspace = $rs; Handle = $handle }
}

function Stop-Sampler($sampler) {
    if ($null -eq $sampler) { return }
    try { Set-Content -Path $StopFile -Value 'stop' -Encoding utf8 } catch { }
    try { Start-Sleep -Milliseconds 300 } catch { }
    try { $sampler.PowerShell.Stop() } catch { }
    try { $sampler.Runspace.Close() } catch { }
    try { Remove-Item $StopFile -Force -ErrorAction SilentlyContinue } catch { }
}

# ---- 构造 cargo 命令 ----
$PluginsManifest = Join-Path $RepoRoot 'plugins\Cargo.toml'
$cargoArgs = @()
$fullRoot = $false

switch ($Task) {
    'fmt' {
        if ($Plugins) {
            $cargoArgs = @('fmt', '--manifest-path', $PluginsManifest, '--all')
        } else {
            $cargoArgs = @('fmt', '--all')
        }
    }
    'clippy' {
        if ($Plugins) {
            $cargoArgs = @('clippy', '--manifest-path', $PluginsManifest, '--all-targets', '--locked', '--jobs', '1', '--', '-D', 'warnings')
        } elseif ($Pkg) {
            $cargoArgs = @('clippy', '-p', $Pkg, '--all-targets', '--all-features', '--locked', '--jobs', '1', '--', '-D', 'warnings')
        } else {
            $fullRoot = $true
            $cargoArgs = @('clippy', '--workspace', '--all-targets', '--all-features', '--locked', '--jobs', '1', '--', '-D', 'warnings')
        }
    }
    'test' {
        if ($Plugins) {
            $cargoArgs = @('test', '--manifest-path', $PluginsManifest, '--locked', '--jobs', '1')
        } elseif ($Pkg) {
            $cargoArgs = @('test', '-p', $Pkg, '--locked', '--jobs', '1')
        } else {
            $fullRoot = $true
            $cargoArgs = @('test', '--workspace', '--locked', '--jobs', '1')
        }
    }
    'build' {
        if ($Plugins) {
            $cargoArgs = @('build', '--manifest-path', $PluginsManifest, '--locked', '--jobs', '1')
        } elseif ($Pkg) {
            $cargoArgs = @('build', '-p', $Pkg, '--locked', '--jobs', '1')
        } else {
            $fullRoot = $true
            $cargoArgs = @('build', '--workspace', '--locked', '--jobs', '1')
        }
    }
    'probe' {
        $cargoArgs = @()
    }
}

if ($fullRoot -and -not $AllowFull) {
    Write-GateLog ("REFUSED allowfull task={0}" -f $Task)
    Write-Host ("INTERCEPT: 本地全量 {0} 需要显式 -AllowFull（本机仅 4.9GB 内存）；" -f $Task) -ForegroundColor Yellow
    Write-Host '  受影响 crate 快速回归请用 -Pkg <crate>，全量优先交 CI。' -ForegroundColor Yellow
    exit 4
}

$mutex = Get-GateMutex
$lockHeld = $false
$sampler = $null
$exitCode = 0

try {
    $abandoned = $false
    try {
        $lockHeld = $mutex.WaitOne(0)
    } catch [System.Threading.AbandonedMutexException] {
        $lockHeld = $true
        $abandoned = $true
    }

    if (-not $lockHeld) {
        Write-GateLog ("BLOCKED task={0} pkg={1} owner=[{2}]" -f $Task, $Pkg, (Get-LeaseOwner))
        Write-Host ("BLOCKED: 编译闸门被占用（{0}）" -f (Get-LeaseOwner)) -ForegroundColor Red
        Write-Host '  不排队等待；等当前编译结束后重试。' -ForegroundColor Red
        $exitCode = 2
        exit $exitCode
    }

    if ($abandoned) { Write-GateLog 'RECOVERED abandoned-mutex' }

    # 陈旧租约回收：拿到锁说明没有活着的持有者
    if (Test-Path $LeaseFile) {
        $stalePid = -1
        try { $stalePid = (Get-Content $LeaseFile -Raw | ConvertFrom-Json).pid } catch { }
        if ($stalePid -ne $PID) {
            Write-GateLog ("STALE reclaimed stalePid={0}" -f $stalePid)
            Write-Host ("RECLAIM: 回收陈旧租约 pid={0}（持有进程已退出）" -f $stalePid) -ForegroundColor DarkYellow
        }
        Remove-Item $LeaseFile -Force -ErrorAction SilentlyContinue
    }

    $lease = [pscustomobject]@{
        pid       = $PID
        startedAt = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        task      = $Task
        pkg       = $Pkg
        plugins   = [bool]$Plugins
        command   = (@('cargo') + $cargoArgs) -join ' '
    }
    $lease | ConvertTo-Json -Compress | Set-Content -Path $LeaseFile -Encoding utf8
    Write-GateLog ("ACQUIRED task={0} pkg={1} plugins={2} cmd=[{3}]" -f $Task, $Pkg, [bool]$Plugins, $lease.command)
    Write-Host ("ACQUIRED: pid={0} task={1} pkg={2}" -f $PID, $Task, $Pkg) -ForegroundColor Green

    if ($Task -eq 'probe') {
        Write-Host ("PROBE: 持锁 {0}s" -f $HoldSeconds)
        $csv = Join-Path $MemDir ((Get-Date -Format 'yyyyMMdd') + '.csv')
        $sampler = Start-Sampler $csv
        Start-Sleep -Seconds $HoldSeconds
        $exitCode = 0
        exit $exitCode
    }

    # 提交内存准入
    try {
        $mem = Get-MemSnapshot
    } catch {
        Write-GateLog 'ADMISSION-WARN counter-unavailable'
        Write-Host 'WARN: 无法读取内存计数器，跳过准入检查' -ForegroundColor DarkYellow
        $mem = $null
    }
    if ($null -ne $mem) {
        Write-Host ("MEM: committed={0}MB limit={1}MB ({2}%) available={3}MB pagefile={4}%" -f `
            $mem.CommittedMB, $mem.LimitMB, $mem.CommitPct, $mem.AvailableMB, $mem.PagefilePct)
        if ($mem.CommitPct -gt $MaxCommitPercent -or $mem.AvailableMB -lt $MinAvailableMB) {
            Write-GateLog ("REFUSED admission committed={0}MB limit={1}MB pct={2} avail={3}MB" -f `
                $mem.CommittedMB, $mem.LimitMB, $mem.CommitPct, $mem.AvailableMB)
            Write-Host ("REFUSED: 提交内存 {0}%（阈值 {1}%）/ 可用 {2}MB（阈值 {3}MB）" -f `
                $mem.CommitPct, $MaxCommitPercent, $mem.AvailableMB, $MinAvailableMB) -ForegroundColor Red
            Write-Host '  请先关闭其他会话或等待当前编译结束，再重试。' -ForegroundColor Red
            $exitCode = 3
            exit $exitCode
        }
    }

    $cmdLine = (@('cargo') + $cargoArgs) -join ' '
    Write-Host ("RUN: {0}" -f $cmdLine) -ForegroundColor Cyan

    if ($DryRun) {
        Write-GateLog ("DRYRUN cmd=[{0}]" -f $cmdLine)
        Write-Host 'DRYRUN: 已通过锁与准入检查，未执行 cargo' -ForegroundColor Cyan
        $exitCode = 0
        exit $exitCode
    }

    $csv = Join-Path $MemDir ((Get-Date -Format 'yyyyMMdd') + '.csv')
    $sampler = Start-Sampler $csv

    # 全量门禁按红线口径关闭增量
    $env:CARGO_INCREMENTAL = '0'
    & cargo @cargoArgs
    $exitCode = $LASTEXITCODE
    Write-GateLog ("FINISHED task={0} pkg={1} exit={2}" -f $Task, $Pkg, $exitCode)
}
finally {
    Stop-Sampler $sampler
    if ($lockHeld) {
        if (Test-Path $LeaseFile) {
            $ownerPid = -1
            try { $ownerPid = (Get-Content $LeaseFile -Raw | ConvertFrom-Json).pid } catch { }
            if ($ownerPid -eq $PID) { Remove-Item $LeaseFile -Force -ErrorAction SilentlyContinue }
        }
        try { $mutex.ReleaseMutex() } catch { }
        Write-GateLog ("RELEASED task={0} exit={1}" -f $Task, $exitCode)
        Write-Host ("RELEASED: pid={0} task={1} exit={2}" -f $PID, $Task, $exitCode) -ForegroundColor Green
    }
    try { $mutex.Dispose() } catch { }
}

exit $exitCode
