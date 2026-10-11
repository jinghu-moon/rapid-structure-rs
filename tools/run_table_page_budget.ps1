# 门禁 G5：页面级表格路由的整页预算测量。
#
# 采集方式：同一台机器、同一份配置、同一批图、同一轮数与线程数，**交替**跑
# "关闭表格路由"与"开启表格路由"，重复 `-Pairs` 次。判据不是两组各自的绝对值，
# 而是**每个配对内部的差值** —— 这样轮间漂移被消掉。
#
# 为什么需要交替：docs/09 §13.5.3 记录了同一配置下 `ocr_total_ms.p50` 的轮间
# 离散度本身就有 9 % 量级（E12 的 R5 扫描）。单次 off/on 对比无法区分
# "表格的开销"与"这一轮恰好偏慢"。
#
# 判定由 `tools/check_table_page_budget.py` 完成；本脚本只负责采集。
#
# 用法：
#   pwsh -NoProfile -File tools/run_table_page_budget.ps1
#   pwsh -NoProfile -File tools/run_table_page_budget.ps1 -Pairs 2 -OutDir Temp\my-g5
#
# 与 `tools/run_windows_baseline.ps1` 不同，本脚本**接受输出目录**且默认写进
# `Temp\table-page-budget`，不会覆盖 `tests/baseline/windows-baseline/` 里随仓库
# 提交的参考报告。

param(
    [string]$Config = '',
    [string]$ImagesDir = '',
    [string]$TableModel = '',
    [string]$OutDir = '',
    [int]$Pairs = 3,
    [int]$Rounds = 3,
    [int]$WarmupRounds = 1,
    [int]$MaxSideLen = 2000,
    [int]$IntraThreads = 16
)

$ErrorActionPreference = 'Stop'
$Crate = Split-Path -Parent $PSScriptRoot
if (-not $Config) { $Config = Join-Path $Crate 'models\configs\test-config-small.yaml' }
if (-not $ImagesDir) { $ImagesDir = Join-Path $Crate 'datasets\text' }
if (-not $TableModel) { $TableModel = Join-Path $Crate 'models\table\slanet-plus.onnx' }
if (-not $OutDir) { $OutDir = Join-Path $Crate 'Temp\table-page-budget' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$Bin = Join-Path $Crate 'target\release\bench_warm_e2e.exe'
foreach ($p in @($Bin, $Config, $ImagesDir, $TableModel)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}
if (-not (Test-Path (Join-Path $ImagesDir 'golden-manifest.json'))) {
    Write-Host "note: $ImagesDir has no golden-manifest.json; the 12-image set is what the budget is defined on"
}

function Invoke-Bench([string]$Out, [switch]$WithTable) {
    $args = @(
        '--config', $Config, '--images-dir', $ImagesDir,
        '--warmup-rounds', "$WarmupRounds", '--rounds', "$Rounds",
        '--max-side-len', "$MaxSideLen", '--intra-threads', "$IntraThreads",
        '--output', $Out
    )
    if ($WithTable) { $args += @('--table-model', $TableModel) }
    $proc = Start-Process -FilePath $Bin -ArgumentList $args -PassThru -NoNewWindow
    $proc.WaitForExit()
    if ($proc.ExitCode -ne 0) { throw "bench_warm_e2e failed (exit $($proc.ExitCode)) for $Out" }
}

Write-Host "=== G5: $Pairs alternating pair(s), max_side_len=$MaxSideLen, rounds=$Rounds ==="
for ($i = 1; $i -le $Pairs; $i++) {
    Write-Host "  pair $i"
    Invoke-Bench (Join-Path $OutDir "pair$i-off.json")
    Invoke-Bench (Join-Path $OutDir "pair$i-on.json") -WithTable
}
Write-Host "wrote $OutDir"
Write-Host "judge with: python tools/check_table_page_budget.py --dir $OutDir"
