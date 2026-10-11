#Requires -Version 5.1
<#
.SYNOPSIS
    serve.ps1 的"就地重画"菜单渲染回归测试（不需要真终端）。

.DESCRIPTION
    为什么需要它：`RAPID_STRUCTURE_SERVE_MENU_INPLACE` 那个测试缝只假装"光标可用"，`Set-MenuCursor`
    故意不认缝，所以无终端环境里跑出来的永远是"降级后的顺序打印"——就地重画这条分支
    （用户实际看到的那一屏）在 CI 里根本跑不到。而用户报的两个渲染问题（选项与说明被挤成
    一行、每次按回车都跳回第一行）恰好只在这条分支里。

    做法：用 PowerShell 的 AST 把 serve.ps1 里的渲染函数**搬出来**（不执行脚本主体），
    把里面的 `[Console]::…` 换成模拟器变量，再用一个假的 `Write-Host` 把每一行写进内存屏幕。
    于是可以在任意"窗口宽度"下断言：
      · 任何一行都不超过窗口（否则真终端会软换行 → 就地重画的行号全错位 → 菜单花屏）；
      · 每个选项各占一行（不会出现"两个选项粘在一行"）；
      · `●` 落在默认选中项那一行；
      · 窗口太窄时**拒绝**就地重画（退回顺序打印，宁可啰嗦不要花屏）。

.EXAMPLE
    pwsh -NoProfile -File tools/test_serve_menu_render.ps1
#>
[CmdletBinding()]
param(
    # 换个 serve.ps1 测：做反向对照（把 Add-MenuPad 换回"按字符数补空格"，这个测试必须 FAIL）。
    [string]$ServePath
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$serve = if ($ServePath) { $ServePath } else { Join-Path $root 'serve.ps1' }
if (-not (Test-Path -LiteralPath $serve -PathType Leaf)) { throw "找不到 serve.ps1：$serve" }

$src = [System.IO.File]::ReadAllText($serve, [System.Text.Encoding]::UTF8)
$tok = $null; $err = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($src, [ref]$tok, [ref]$err)
if ($err.Count -gt 0) { throw "serve.ps1 语法错误：$(($err | ForEach-Object { $_.Message }) -join ' | ')" }

function Get-FnAst([string]$name) {
    $found = @($ast.FindAll({ param($n)
        $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true))
    if ($found.Count -eq 0) { throw "serve.ps1 里找不到函数 $name" }
    return $found[0]
}

# 就地重画那条分支要读的控制台尺寸/光标：换成模拟器里的变量，其余逻辑一个字不动。
$patch = @{
    '[Console]::WindowWidth'    = '$script:SimW'
    '[Console]::WindowHeight'   = '$script:SimH'
    '[Console]::BufferHeight'   = '$script:SimBH'
    '[Console]::CursorVisible'  = '$script:SimCursorVisible'
    '[Console]::CursorTop'      = '$script:SimRow'
}
function Get-PatchedText([string]$name) {
    $text = (Get-FnAst $name).Extent.Text
    foreach ($k in $patch.Keys) { $text = $text.Replace($k, $patch[$k]) }
    return $text
}
# 注意：Invoke-Expression 必须在**脚本顶层**执行。放进函数里定义，函数一返回这些函数就随
# 局部作用域一起消失（假 Write-Host 里调用 Get-DisplayWidth 会报"无法识别的术语"）。
foreach ($f in 'Get-DisplayWidth', 'Add-MenuPad', 'Move-MenuIndex', 'Show-ArrowMenu') {
    Invoke-Expression (Get-PatchedText $f)
}

# 本脚本自己的输出必须绕过假的 Write-Host（下面会把 Write-Host 换成内存屏幕）。
function Say([string]$text = '') { [Console]::Out.WriteLine($text) }

# ---------------------------------------------------------------- 内存屏幕 + 假控制台
$script:SimW = 120
$script:SimH = 40
$script:SimBH = 400
$script:SimRow = 0
$script:SimCol = 0
$script:SimCursorVisible = $true
$script:ResetOnNextWrite = $false
$script:Screen = @{}          # 行号 → 该行文字
$script:CursorCalls = 0       # 就地重画次数（0 = 走了顺序打印）
$script:Keys = @()

function Write-Host {
    param(
        [Parameter(Position = 0, ValueFromRemainingArguments = $true)]$Object,
        [switch]$NoNewline,
        [string]$ForegroundColor,
        [switch]$BackgroundColor
    )
    $line = if ($null -eq $Object) { '' } elseif ($Object -is [array]) { ($Object | ForEach-Object { [string]$_ }) -join ' ' } else { [string]$Object }
    if ($script:ResetOnNextWrite) { $script:Screen[$script:SimRow] = ''; $script:ResetOnNextWrite = $false }
    if (-not $script:Screen.ContainsKey($script:SimRow)) { $script:Screen[$script:SimRow] = '' }
    $script:Screen[$script:SimRow] = [string]$script:Screen[$script:SimRow] + $line
    $script:SimCol += (Get-DisplayWidth $line)
    if (-not $NoNewline) { $script:SimRow++; $script:SimCol = 0 }
}

function Set-MenuCursor { param([int]$Row)
    $script:CursorCalls++
    $script:SimRow = $Row; $script:SimCol = 0; $script:ResetOnNextWrite = $true
    return $true
}
function Get-MenuCursorTop { return $script:SimRow }
function Clear-MenuScreen { $script:Screen = @{}; $script:SimRow = 0; $script:SimCol = 0; return $true }
function Test-ConsoleCursor { return $true }
function Read-MenuKey { if ($script:Keys.Count -eq 0) { return 'Escape' }; $k = $script:Keys[0]; $script:Keys = @($script:Keys | Select-Object -Skip 1); return $k }

# 与 Select-ServeProfile 里的那一屏同形（标题 + 副标题 + 三个选项，全是中英混排的长说明）。
$items = @(
    @{ Key = 'quick'; Label = '快速档'; Badge = '推荐'; Hint = 'small + 表格 · 本机每页约 0.9 s'; Disabled = $false },
    @{ Key = 'full'; Label = '全功能档'; Badge = '当前选择'; Hint = 'medium + 公式 + 表格 · 本机每页约 18–27 s'; Disabled = $false },
    @{ Key = 'back'; Label = '返回'; Badge = ''; Hint = $null; Disabled = $false }
)
$subtitle = '当前档位已预选；选完立刻生效（下一次启动 / 检查用它）。Esc 返回。'

$fail = 0
function Assert-That([string]$what, [bool]$ok, [string]$detail) {
    if (-not $ok) { $script:fail++ }
    Say ('  [{0}] {1}{2}' -f $(if ($ok) { 'OK  ' } else { 'FAIL' }), $what, $(if ($detail) { " —— $detail" } else { '' }))
}

function Invoke-Screen([int]$width, [int]$height, [string[]]$keys) {
    $script:SimW = $width; $script:SimH = $height; $script:SimBH = 400
    $script:SimRow = 0; $script:SimCol = 0; $script:Screen = @{}; $script:CursorCalls = 0; $script:Keys = $keys
    $script:MenuLastIndex = $null
    $script:ShowResult = Show-ArrowMenu -Title '切换档位' -Subtitle $subtitle -Items $items -DefaultIndex 1
    return $script:Screen
}
function Get-MarkerRows([hashtable]$screen) {
    @($screen.Keys | Sort-Object | Where-Object { $screen[$_] -match '^\s+[●○] ' })
}

Say ('serve.ps1 : {0}' -f $serve)
Say ('抽出函数 : Get-DisplayWidth / Add-MenuPad / Move-MenuIndex / Show-ArrowMenu（[Console]:: 已换成模拟器）')

# ---------------------------------------------------------------- 用例 1：正常宽度（就地重画）
Say ''
Say '用例 1：窗口 120×40 —— 必须就地重画，且任何一行都不超出窗口'
[void](Invoke-Screen 120 40 @('Enter'))
$screen = $script:Screen
$widths = @($screen.Keys | Where-Object { $screen[$_] -ne '' } | ForEach-Object { Get-DisplayWidth $screen[$_] })
$maxw = ($widths | Measure-Object -Maximum).Maximum
Assert-That '确实走了就地重画（有光标定位）' ($script:CursorCalls -gt 0) "Set-MenuCursor 调用 $($script:CursorCalls) 次"
Assert-That '没有任何一行达到/超过窗口宽度' ($maxw -le 119) "最宽 $maxw 列（窗口 120）"
$markers = Get-MarkerRows $screen
Assert-That '三个选项各占一行' ($markers.Count -eq 3) "带标记的行 = $($markers.Count)"
$glued = @($markers | Where-Object { ([regex]::Matches($screen[$_], '[●○] ')).Count -gt 1 })
Assert-That '没有"两个选项粘在同一行"' ($glued.Count -eq 0) $(if ($glued.Count) { "行 $($glued -join ',')" } else { '' })
$selRows = @($markers | Where-Object { $screen[$_] -match '^\s+● ' })
Assert-That '● 只在默认选中项那一行' ($selRows.Count -eq 1 -and $screen[$selRows[0]] -match '全功能档') $(if ($selRows.Count) { $screen[$selRows[0]].Trim() } else { '没有 ●' })
Assert-That '返回键回传 Key' ($script:ShowResult -eq 'full') "返回 = '$($script:ShowResult)'"
Assert-That '记下了光标停在哪一行' ($script:MenuLastIndex -eq 1) "MenuLastIndex = $($script:MenuLastIndex)"
Say '  屏幕（就地重画那帧，行号 = 终端行）：'
foreach ($r in ($screen.Keys | Sort-Object)) { if ($screen[$r] -ne '') { Say ('    {0,3}|{1}|' -f $r, $screen[$r]) } }

# ---------------------------------------------------------------- 用例 2：真实交互序列（Down 移动 + Enter）
Say ''
Say '用例 2：窗口 120×40 —— Down 之后 Enter，选中项必须跟着走'
[void](Invoke-Screen 120 40 @('Down', 'Enter'))
$m = Get-MarkerRows $script:Screen
$sel = @($m | Where-Object { $script:Screen[$_] -match '^\s+● ' })
Assert-That 'Down 把 ● 移到了"返回"' ($sel.Count -eq 1 -and $script:Screen[$sel[0]] -match '返回') $(if ($sel.Count) { $script:Screen[$sel[0]].Trim() } else { '没有 ●' })
Assert-That 'Enter 回传的就是移动后的那一项' ($script:ShowResult -eq 'back') "返回 = '$($script:ShowResult)'"

# ---------------------------------------------------------------- 用例 3：窄窗口必须降级
Say ''
Say '用例 3：窗口 40×40 —— 一行放不下时必须拒绝就地重画（顺序打印，绝不花屏）'
[void](Invoke-Screen 40 40 @('Escape'))
Assert-That '窄窗口没有就地重画' ($script:CursorCalls -eq 0) "Set-MenuCursor 调用 $($script:CursorCalls) 次"
Assert-That 'Esc 返回 $null' ($null -eq $script:ShowResult) ''

# ---------------------------------------------------------------- 用例 4：矮窗口必须降级
Say ''
Say '用例 4：窗口 200×8 —— 选项区放不下时也必须降级（以前的行号越界就是这么来的）'
[void](Invoke-Screen 200 8 @('Escape'))
Assert-That '矮窗口没有就地重画' ($script:CursorCalls -eq 0) "Set-MenuCursor 调用 $($script:CursorCalls) 次"

Say ''
Say ('FAILURES = {0}' -f $fail)
exit $(if ($fail -eq 0) { 0 } else { 1 })
