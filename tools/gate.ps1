<#
单写者编译闸门。本机所有 cargo fmt / clippy / test / build 的唯一入口。

设计要点（2026-11-07 卡死事故复盘落地）：
  1. Windows 命名互斥体 Global\SkiffBuild：全机同一时刻只允许一路编译，
     进程死亡自动释放，不会留下无法判断存活的目录锁。
  2. 租约文件 .workbuddy/locks/build.lease：记录 pid / 会话 / 命令 / 开始时间，
     仅持锁进程可在 finally 中释放；陈旧租约自动回收并写日志。
  3. 拿不到锁立即以退出码 2 结束，不排队、不等待。
  4. 根 workspace 与 plugins/ 共用同一把互斥体，禁止并行。
  5. 内存准入：可用物理内存 < 1024MB 时拒绝启动（退出码 3），下限可用
     SKIFF_GATE_MIN_AVAILABLE_MB 调整。本机是动态内存虚拟机，commit limit 在
     4.7–7.2GB 间浮动，提交量/百分比阈值没有判别力。
  6. 运行期间每 5 秒采样一次，写入 .workbuddy/memwatch/YYYYMMDD.csv，保留 7 天。
  7. 本地全量拦截：根 workspace 的 clippy / test / build 不带 -Pkg 时必须显式 -AllowFull（退出码 4）。
  8. git 写命令（add / commit / merge / rebase / reset / checkout / switch / restore / stash /
     gc / prune / mv / rm / tag / push / fetch 等）与 cargo 共用同一把互斥体，租约 workspace=git；
     只读命令（status / log / diff / show 等）不占令牌，但写审计（AUDIT git-read）。

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
    [ValidateSet('fmt', 'clippy', 'test', 'build', 'exec', 'git', 'probe')]
    [string]$Task,

    # 仅 -Task exec：由 PATH 包装入口透传的原始 cargo 参数。
    # 命名必须与内部数组 $cargoArgs 区分开：PowerShell 变量名大小写不敏感，
    # 同名会被后面的 $cargoArgs = @() 直接覆盖（曾导致所有参数被清空）。
    [string[]]$ExecArgs = @(),

    # 仅 -Task git：由 tools/bin/git.cmd 透传的原始 git 参数
    [string[]]$GitArgs = @(),

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

$MinAvailableMB   = 1024
$RetentionDays    = 7
if ($env:SKIFF_GATE_MIN_AVAILABLE_MB) {
    $parsed = 0
    if ([int]::TryParse($env:SKIFF_GATE_MIN_AVAILABLE_MB, [ref]$parsed) -and $parsed -gt 0) {
        $MinAvailableMB = $parsed
    }
}

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
        FreeCommitMB = [math]::Round(($s[1].CookedValue - $s[0].CookedValue) / 1MB, 0)
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

function Resolve-RealCargo {
    if ($env:SKIFF_REAL_CARGO -and (Test-Path $env:SKIFF_REAL_CARGO)) { return $env:SKIFF_REAL_CARGO }
    foreach ($p in ($env:PATH -split ';')) {
        if (-not $p) { continue }
        $exe = Join-Path $p 'cargo.exe'
        if (Test-Path $exe) { return $exe }
    }
    $fallback = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path $fallback) { return $fallback }
    throw '找不到真实的 cargo.exe（PATH 中除包装入口外没有 cargo.exe）'
}

function Resolve-RealGit {
    if ($env:SKIFF_REAL_GIT -and (Test-Path $env:SKIFF_REAL_GIT)) { return $env:SKIFF_REAL_GIT }
    foreach ($p in ($env:PATH -split ';')) {
        if (-not $p) { continue }
        $exe = Join-Path $p 'git.exe'
        if (Test-Path $exe) { return $exe }
    }
    $fallback = Join-Path $env:ProgramFiles 'Git\cmd\git.exe'
    if (Test-Path $fallback) { return $fallback }
    throw '找不到真实的 git.exe（PATH 中除包装入口外没有 git.exe）'
}

# ---- 构造 cargo 命令 ----
$PluginsManifest = Join-Path $RepoRoot 'plugins\Cargo.toml'
$cargoArgs = @()
$fullRoot = $false
$isGit = $false

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
    'exec' {
        # PATH 包装透传的原始参数；-Pkg/-Plugins 不参与
        $cargoArgs = @($ExecArgs)
    }
    'git' {
        # PATH 包装透传的原始 git 参数；只读命令放行但仍记审计
        $cargoArgs = @($GitArgs)
        $isGit = $true
    }
    'probe' {
        $cargoArgs = @()
    }
}

# 根 workspace 与 plugins/ 共用同一把令牌，这里只标注归属用于审计
$wsKind = 'root'
if ($Plugins) { $wsKind = 'plugins' }
if ($isGit) { $wsKind = 'git' }
if ($Task -eq 'exec') {
    for ($i = 0; $i -lt $cargoArgs.Count - 1; $i++) {
        if ($cargoArgs[$i] -eq '--manifest-path' -and $cargoArgs[$i + 1] -match 'plugins') { $wsKind = 'plugins' }
    }
    foreach ($a in $cargoArgs) {
        if ($a -match '^--manifest-path=.*plugins') { $wsKind = 'plugins' }
    }
}

if ($fullRoot -and -not $AllowFull) {
    Write-GateLog ("REFUSED allowfull task={0}" -f $Task)
    Write-Host ("INTERCEPT: 本地全量 {0} 需要显式 -AllowFull（本机仅 4.9GB 内存）；" -f $Task) -ForegroundColor Yellow
    Write-Host '  受影响 crate 快速回归请用 -Pkg <crate>，全量优先交 CI。' -ForegroundColor Yellow
    exit 4
}

# ---- git：只读命令直接放行（不占令牌，但记审计）；写命令与 cargo 共用同一把令牌 ----
if ($isGit) {
    $gitVerb = ''
    $i = 0
    while ($i -lt $cargoArgs.Count) {
        $t = $cargoArgs[$i]
        if ($t -in @('-C', '-c', '--git-dir', '--work-tree', '--namespace')) { $i += 2; continue }
        if ($t -like '-*') { $i += 1; continue }
        $gitVerb = $t
        break
    }
    $readOnlyVerbs = @(
        'status', 'log', 'diff', 'show', 'rev-parse', 'rev-list', 'ls-files', 'ls-remote',
        'describe', 'blame', 'shortlog', 'cat-file', 'show-ref', 'for-each-ref', 'grep',
        'whatchanged', 'count-objects', 'fsck', 'check-ignore', 'check-attr', 'merge-base',
        'name-rev', 'symbolic-ref', 'version', 'help'
    )
    if (($readOnlyVerbs -contains $gitVerb) -or ($cargoArgs -contains '--version')) {
        $realGitRO = Resolve-RealGit
        $roLine = (@($realGitRO) + $cargoArgs) -join ' '
        Write-GateLog ("AUDIT git-read verb={0} cmd=[{1}]" -f $gitVerb, $roLine)
        # 只读命令不刷新 index（否则 status 也会写 .git/index，制造写冲突）
        $env:GIT_OPTIONAL_LOCKS = '0'
        & $realGitRO @cargoArgs
        exit $LASTEXITCODE
    }
    if (-not $gitVerb) {
        Write-GateLog 'REFUSED git-empty-args'
        Write-Host 'REFUSED: 未识别的 git 参数（需要子命令）' -ForegroundColor Red
        exit 2
    }
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

    $toolName = if ($isGit) { 'git' } else { 'cargo' }
    $lease = [pscustomobject]@{
        pid       = $PID
        startedAt = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        task      = $Task
        workspace = $wsKind
        pkg       = $Pkg
        plugins   = [bool]$Plugins
        command   = (@($toolName) + $cargoArgs) -join ' '
    }
    $lease | ConvertTo-Json -Compress | Set-Content -Path $LeaseFile -Encoding utf8
    Write-GateLog ("ACQUIRED task={0} workspace={1} pkg={2} cmd=[{3}]" -f $Task, $wsKind, $Pkg, $lease.command)
    Write-Host ("ACQUIRED: pid={0} task={1} workspace={2} pkg={3}" -f $PID, $Task, $wsKind, $Pkg) -ForegroundColor Green

    if ($Task -eq 'probe') {
        Write-Host ("PROBE: 持锁 {0}s" -f $HoldSeconds)
        $csv = Join-Path $MemDir ((Get-Date -Format 'yyyyMMdd') + '.csv')
        $sampler = Start-Sampler $csv
        Start-Sleep -Seconds $HoldSeconds
        $exitCode = 0
        exit $exitCode
    }

    # 提交内存准入（仅 cargo；git 写不占内存峰值）
    if (-not $isGit) {
        try {
            $mem = Get-MemSnapshot
        } catch {
            Write-GateLog 'ADMISSION-WARN counter-unavailable'
            Write-Host 'WARN: 无法读取内存计数器，跳过准入检查' -ForegroundColor DarkYellow
            $mem = $null
        }
        if ($null -ne $mem) {
            Write-Host ("MEM: available={0}MB（准入下限 {1}MB） committed={2}MB limit={3}MB pagefile={4}%（仅供观测）" -f `
                $mem.AvailableMB, $MinAvailableMB, $mem.CommittedMB, $mem.LimitMB, $mem.PagefilePct)
            # 动态内存虚拟机：commit limit 在 4.7–7.2GB 浮动，比例阈值不可靠，只看可用物理内存绝对下限
            if ($mem.AvailableMB -lt $MinAvailableMB) {
                Write-GateLog ("REFUSED admission available={0}MB min={1}MB" -f $mem.AvailableMB, $MinAvailableMB)
                Write-Host ("REFUSED: 可用物理内存 {0}MB 低于下限 {1}MB（可用 SKIFF_GATE_MIN_AVAILABLE_MB 调整）" -f `
                    $mem.AvailableMB, $MinAvailableMB) -ForegroundColor Red
                Write-Host '  请先关闭其他会话或等待当前编译结束，再重试。' -ForegroundColor Red
                $exitCode = 3
                exit $exitCode
            }
        }
    }

    # 绕过 PATH 包装递归：始终调用真实可执行文件
    $realExe = if ($isGit) { Resolve-RealGit } else { Resolve-RealCargo }
    $cmdLine = (@($realExe) + $cargoArgs) -join ' '
    Write-Host ("RUN: {0}" -f $cmdLine) -ForegroundColor Cyan

    if ($DryRun) {
        Write-GateLog ("DRYRUN cmd=[{0}]" -f $cmdLine)
        Write-Host 'DRYRUN: 已通过锁与准入检查，未执行' -ForegroundColor Cyan
        $exitCode = 0
        exit $exitCode
    }

    if (-not $isGit) {
        $csv = Join-Path $MemDir ((Get-Date -Format 'yyyyMMdd') + '.csv')
        $sampler = Start-Sampler $csv
        # 门禁任务按红线口径关闭增量；包装透传保持调用方语义
        if ($Task -ne 'exec') { $env:CARGO_INCREMENTAL = '0' }
    }
    & $realExe @cargoArgs
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
