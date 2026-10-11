#Requires -Version 5.1
<#
.SYNOPSIS
    一条命令启动本地网页服务（`rapidstruct serve`），并检查/补齐它需要的模型。

.DESCRIPTION
    模型参数不写在本脚本里，而在一个 profile 文件里。仓库里有两个：

        models/configs/serve-local.json        快档（默认）：models/text/small + 表格
        models/configs/serve-local-full.json    全功能档：models/serve（medium + 公式）+ 表格

    快档是本脚本的内建默认（也就是 `.\serve.ps1` 不带参数时的档位）：本机实测
    同一张 3200x2000 页面 0.87 s（small），而全功能档 18–27 s（medium）——同一台
    机器上 medium 的 infer 成本约为 small 的 20 倍（线程扫描 t=1 138 s / t=16 15.8 s，
    所以不是线程配置问题，是模型本身的量级差）。代价是快档没有公式 role，
    公式路由因此被禁用（页面里「文本 + 公式」两项会变灰并说明原因）。

    用 `.\serve.ps1 -ServeConfig models\configs\serve-local-full.json` 切到全功能档。

    profile 字段：

        model_dir       模型集目录。若 `<该目录>/manifest.json` 存在，它是该目录的
                        **唯一权威描述**（引擎配置里的 det/cls/rec 模型路径会被它
                        按 role 覆盖）；不存在则退回 `assets/default_models.yaml`
                        的内建默认表，文件名按 EngineConfig 里的 model_type 推导
                        （models/text/small 就是这种情况）。
                        **给某个目录写 manifest.json 之前先看这条**：serve 的 ModelRequest
                        恒为 text_and_formula（`src/bin/serve/model_plan.rs:261`：库存要能报告
                        公式集合的体积/哈希并按集合下载，"运行计划是另一件事"），所以本地清单
                        只产生**一个**集合、且必须同时声明 detector / recognizer / dictionary /
                        formula_recognizer 四个 role（models/serve 就是五个 role 全在）。
                        因此「只有文本 + 表格」的档位不该写 manifest.json：退回内建默认表时，
                        /api/models 会把内建表声明的公式集合（PP-FormulaNet_plus-M，594 MB）
                        单列成"缺失"——那是刻意留出的下载入口，**不是本档位的故障**。
        config          EngineConfig YAML（推理参数：阈值、max_side_len、线程…）
        table_model     表格结构模型；它的 SHA-256 来自模型自己旁边的
                        models/table/manifest.json（表格的登记放在模型旁边，
                        不放进 models/serve，见 docs/07 §7.2 的理由）
        table_enabled   是否启用表格路由（等价于 -NoTable）
        port            监听端口（`rapidstruct serve` 默认 8760，被占用时报错不换端口）
        open            启动后是否打开系统默认浏览器
        provider        可选 cpu | directml | cuda；缺省跟随 config 的 provider_preference

    优先级：**命令行参数 > profile 文件 > 本脚本内建默认值**。菜单只负责选一个
    profile 和少数几个可选覆盖项，不引入第二套优先级。

    ---- 交互菜单（无参数调用，或 -Menu） ----

        .\serve.ps1             没有参数 → 进方向键菜单（RapidStructure 服务管理器）
        .\serve.ps1 -Menu       显式进菜单

    菜单：启动本地服务 / 检查 · 下载模型 / 环境与配置诊断 / 高级选项 / 退出。
    键位：↑↓ 移动、Enter 确认、Esc 返回（在主菜单里退出）、Home/End 跳首尾。
    换档位：主菜单两个动作启动前会问一次；也可以在「高级选项 → 档位」里先切好
    （主菜单首项的徽标会写「档位：…（已切换）」）。上次成功启动的档位会被记住并预选。
    一律做**选择题**：每个决定都列出可选项，推荐项标「推荐」并默认选中（连"确认启动"
    和"要不要下载"都是选择题）；唯一需要手动输入的是「高级选项 → 端口 → 自定义端口…」。
    每一屏先清屏再画（按 Enter 之后不会与上一屏的文字堆在一起）；选项区用光标定位
    就地重画，所以不闪也不刷屏；没有鼠标支持、没有动画、不依赖任何第三方 TUI 库；
    输出被重定向（Agent / 管道 / 重定向日志）时自动退回顺序打印（此时不清屏），不报错。

    **只要给了任何原有操作参数，就走原来的命令行路径（不弹菜单）**：
    -ServeConfig / -Port / -ModelDir / -Config / -TableModel / -Provider /
    -NoTable / -NoOpen / -Reverify / -NoBuild / -DryRun / -Download /
    -NoDownload / -PreferMirror / -ModelsOnly。其中 `-ModelsOnly` 与 `-Menu` 同给时
    以 `-ModelsOnly` 为准（它是操作参数，不是选项）。`-Menu` 与其它参数同给时那些
    参数仍然生效，并在菜单里标成「由命令行参数固定」，不让菜单把它们覆盖掉。

    终端不可交互（stdin/stdout 被重定向、Agent 或管道调用）时：
      · 无参数 → **不**弹菜单，按内建默认档位走命令行路径（与以前完全一致，绝不等待按键）；
      · 显式 -Menu → 打印原因并退出 1（不启动任何东西，也不卡住）。

    `-Download` / `-NoDownload` 的语义与以前一致：前者预先授权，后者从不询问；
    「只问一次」的授权状态仍然只由 Test-DownloadConsent 一处统一判定，菜单里的
    检查/下载也走它，不存在第二个授权入口。

    ---- UI 状态（只记界面偏好） ----

        %LOCALAPPDATA%\RapidStructure\serve-ui.json = {"version":1,"lastProfile":"quick"}

    只记「上一次**成功启动**的档位」，用来决定菜单里默认选中哪一项。它不写进仓库、
    不覆盖任何 profile 字段、也不覆盖命令行参数（命令行给了 -ServeConfig 时菜单连
    档位选择都不问）。缺失 → 用快速档且不报错；损坏或版本不认识 → 忽略内容、回退
    快速档并提示一次；保存失败 → 只警告，不影响服务。**浏览菜单、取消操作、只做模型
    检查都不会写它**，只有确认启动且服务真的就绪之后才写（先写 .tmp 再原子替换）。

    ---- 自动化测试缝（仅供本脚本自己的回归测试） ----

    显式传 -Menu 时，下面两个环境变量可以注入按键与输入，让菜单在无终端环境下也能
    被逐帧驱动；普通调用（含无参数调用）永远读真实键盘：
      RAPID_STRUCTURE_SERVE_MENU_KEYS=Down,Enter,Escape   按键队列（Up/Down/Enter/Escape/Home/End）
      RAPID_STRUCTURE_SERVE_MENU_VALUES=8790,cuda         输入队列，逗号或 | 分隔
    队列耗尽即视为 Esc / 用默认值，绝不会阻塞。

    ---- 模型检查与下载（-ModelsOnly / -Download） ----

    发布的包里不带模型，所以本脚本在启动服务**之前**先盘点一次：

      1. 真启动一次 `rapidstruct serve`（后台），读它的 `GET /api/models`。
         这是**唯一**权威来源：它已经把三种模型——OCR（detector/recognizer/dictionary）、
         表格（table_structure）、公式（formula_recognizer/formula_detector）——的
         逐文件 `state` / `size_bytes` / `sha256` / `source_url` 合在一起回答了。
         本脚本**不**自己去解析 `<dir>/manifest.json` 或 `assets/default_models.yaml`：
         内建默认表是 YAML，而且同一份事实如果在这里再实现一遍，两处必然漂移。
      2. 缺文件时打印「管线 / 文件 / 角色 / 大小 / 下载地址 / 保存为 / SHA-256」，
         让用户手动下载；或经同意后（`-Download`，或交互式确认）自动下载到
         各自的目录并逐个校验 SHA-256（校验不过就删掉临时文件并报错）。
      3. 只有**运行计划里**的文件缺失才算缺：内建默认表声明的公式集合（594 MB）在
         快档里不参与运行，所以它出现在页面清单里、但不影响启动。

    只有清单声明了 `source_url` 的文件才能自动下载；没有声明时脚本如实说
    "清单未声明下载地址"，不编造 URL。允许的域名 = 库的白名单（`src/model_store.rs`）
    加上官方托管 Pix2Text 公式检测模型的 Hugging Face `huggingface.co` 与它的国内镜像
    `hf-mirror.com`（也认 HF_ENDPOINT 环境变量指向的镜像 host）。

    Hugging Face 的链接**同时打印官方地址与镜像地址**；下载时先试官方、**传输失败**才换镜像
    （`-PreferMirror` 反过来，镜像在前，适合国内网络）。**SHA-256 不匹配绝不换源重试**：
    真正的兜底是清单声明的逐字节校验，不是域名。

    `-ModelsOnly` 只做检查/下载然后退出，不留在前台服务。

    与手敲长命令的关系：本脚本只做四件事——读 profile、把相对路径按仓库根目录
    展开、必要时构建（`--features serve`）、按 `rapidstruct serve` 的旗标拼出命令。
    它**不**新增任何启动语义，所以 `-DryRun` 打印出来的那一行可以原样手敲；
    模型检查走的是服务自己的 API，也没有绕开它的校验。

.EXAMPLE
    .\serve.ps1
    按 models/configs/serve-local.json 启动（文本 + 表格，快档）。

.EXAMPLE
    .\serve.ps1 -ServeConfig models\configs\serve-local-full.json
    全功能档：文本 + 公式 + 表格（medium + formula，本机每页约 18–27 s）。

.EXAMPLE
    .\serve.ps1 -ModelsOnly
    只检查模型是否齐备：缺就打印下载地址，齐备就退出 0。不留在前台服务。

.EXAMPLE
    .\serve.ps1 -Download
    预先同意自动下载缺失模型（逐个核对清单里的 SHA-256），下完继续启动服务。

.EXAMPLE
    .\serve.ps1 -Port 8790 -NoOpen
    换端口、不开浏览器。

.EXAMPLE
    .\serve.ps1 -NoTable -Provider cpu
    只跑文本管线（不启用表格路由），并显式指定 CPU。

.EXAMPLE
    .\serve.ps1 -DryRun
    只打印将要执行的命令：不构建、不启动。

.EXAMPLE
    .\serve.ps1 -ServeConfig models\configs\serve-local-full.json -PreferMirror -Download
    全功能档，并让 Hugging Face 的文件先走国内镜像 hf-mirror.com（失败才试官方）。

.EXAMPLE
    .\serve.ps1 -Reverify --max-side 1280
    启动期冷验证全部计划模型（逐文件核对清单里的 SHA-256）；`--max-side` 这样
    本脚本不认识的旗标会原样追加给 `rapidstruct serve`。

.EXAMPLE
    .\serve.ps1
    没有参数：进方向键菜单（也可以用 -Menu 显式进）。菜单里选档位、检查模型、
    看诊断，最后确认启动；上次成功启动的档位会记在
    %LOCALAPPDATA%\RapidStructure\serve-ui.json 里，下次默认选中它。
#>
[CmdletBinding()]
param(
    # 另一个 profile（JSON；相对路径按仓库根目录解析）。
    [string]$ServeConfig,
    # 监听端口；覆盖 profile。
    [int]$Port,
    # 模型集目录；覆盖 profile。
    [string]$ModelDir,
    # EngineConfig YAML；覆盖 profile。
    [string]$Config,
    # 表格结构模型；覆盖 profile。
    [string]$TableModel,
    # 覆盖 provider。
    [ValidateSet('cpu', 'directml', 'cuda')]
    [string]$Provider,
    # 不启用表格路由。
    [switch]$NoTable,
    # 不打开浏览器。
    [switch]$NoOpen,
    # 启动期冷验证计划里的每个模型（读一遍 + 核对 SHA-256）。
    [switch]$Reverify,
    # 不构建，直接用现有的 target\release\rapidstruct.exe。
    [switch]$NoBuild,
    # 只打印将要执行的命令。
    [switch]$DryRun,
    # 预先同意自动下载缺失的模型（不问）。
    [switch]$Download,
    # 从不下载、也不询问（缺模型时只打印地址）。
    [switch]$NoDownload,
    # 下载 Hugging Face 的文件时优先用镜像（hf-mirror.com），原始域名作后备。
    [switch]$PreferMirror,
    # 只做模型检查/下载，不留在前台服务。
    [switch]$ModelsOnly,
    # 显式进入交互菜单（无参数调用同样进菜单；给了原有操作参数则走命令行路径）。
    [switch]$Menu,
    # 等待服务就绪的秒数。
    [int]$ReadyTimeoutSec = 60,
    # 原样追加给 `rapidstruct serve` 的其它旗标。
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = 'Stop'

$root = $PSScriptRoot
$exe = Join-Path $root 'target\release\rapidstruct.exe'

# 交互菜单里的档位：稳定的内部标识 → profile 相对路径。菜单只显示标识与中文名，
# 不显示路径；状态文件里也只存标识（换文件名时只改这里）。加档位就在这里加一行。
$ProfileMap = [ordered]@{
    quick = 'models\configs\serve-local.json'
    full  = 'models\configs\serve-local-full.json'
}
$ProfileLabels = @{ quick = '快速档'; full = '全功能档' }
$ProfileHints = @{
    quick = 'small + 表格 · 本机每页约 0.9 s'
    full  = 'medium + 公式 + 表格 · 本机每页约 18–27 s'
}
$DefaultProfileId = 'quick'
$defaultProfile = $ProfileMap[$DefaultProfileId]

# 与 src/model_store.rs 的 ALLOWED_DOWNLOAD_HOSTS 一致，另加官方托管 Pix2Text 公式检测模型的
# Hugging Face 域名**与其国内镜像**（模型清单里声明、由 SHA-256 逐字节兜底）。清单声明了别的
# 域名时本脚本只跳过并提示手动下载。注意服务端 /api/models/download 那条路径仍只认库自己的
# 白名单——需要页面下载时用 serve 的 --allow-download-host 追加。
$AllowedDownloadHosts = @('www.modelscope.cn', 'cdn-lfs-cn-1.modelscope.cn', 'huggingface.co', 'hf-mirror.com')

# 镜像：同路径换主机（HF 官方 ↔ 国内镜像）。顺带认 Hugging Face 生态约定的 HF_ENDPOINT
# 环境变量（例如 https://hf-mirror.com），它比内置镜像优先，并相应追加进允许域名。
# 候选地址只在**传输**失败时才换下一个；SHA-256 不匹配绝不换源重下（否则就成了
# "挑一个能对上哈希的源"——那就等于没有完整性校验）。
$HostMirrors = @{ 'huggingface.co' = 'hf-mirror.com'; 'hf-mirror.com' = 'huggingface.co' }
$hfEndpointHost = $null
if ($env:HF_ENDPOINT) {
    try { $hfEndpointHost = ([uri]$env:HF_ENDPOINT).Host } catch { $hfEndpointHost = $null }
    if ($hfEndpointHost) {
        $HostMirrors['huggingface.co'] = $hfEndpointHost
        if ($AllowedDownloadHosts -notcontains $hfEndpointHost) {
            # 不静默放宽：Hugging Face 生态的镜像变量由使用者自己设置，内容仍由清单里的
            # SHA-256 逐字节兜底，所以允许，但要说出来。
            $AllowedDownloadHosts += $hfEndpointHost
            Write-Host ("  HF_ENDPOINT 指向 {0}：已追加为允许域名（内容仍按清单的 SHA-256 校验）" -f $hfEndpointHost) -ForegroundColor DarkYellow
        }
    }
}

function Get-MirrorUrl {
    param([string]$Url)
    try { $u = [uri]$Url } catch { return $null }
    if (-not $HostMirrors.ContainsKey($u.Host)) { return $null }
    $builder = [System.UriBuilder]::new($u)
    $builder.Host = $HostMirrors[$u.Host]
    return $builder.Uri.AbsoluteUri
}

# 一个文件的候选下载地址 = 原始地址 + 镜像（镜像的 host 必须也在允许域名内才算数）。
# -PreferMirror 时镜像在前（国内网络下 HF 官方常连不上）。
function Get-DownloadCandidates {
    param([string]$Url)
    $mirror = Get-MirrorUrl -Url $Url
    if (-not $mirror) { return @($Url) }
    try { $mirrorHost = ([uri]$mirror).Host } catch { return @($Url) }
    if ($AllowedDownloadHosts -notcontains $mirrorHost) { return @($Url) }
    if ($PreferMirror) { return @($mirror, $Url) }
    return @($Url, $mirror)
}

$PipelineZh = @{ text = 'OCR 文本'; formula = '公式'; table = '表格' }

# 读 JSON 一律走这里。**不要用 `Get-Content -Raw`**：Windows PowerShell 5.1 的 `Get-Content`
# 在没有 BOM 时按系统 ANSI 代码页解码，而 profile 文件里有中文 `_comment` —— 解出来是乱码，
# `ConvertFrom-Json` 直接报 `Invalid object passed in, ':' or '}' expected`（实测 5.1.26100）。
# 显式按 UTF-8 读；BOM 由 ReadAllText 自己处理。
function Read-Utf8Json {
    param([string]$Path)
    $text = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
    if ($text.Length -gt 0 -and [int][char]$text[0] -eq 0xFEFF) { $text = $text.Substring(1) }
    return ($text | ConvertFrom-Json)
}

function Resolve-RepoPath {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    if ([System.IO.Path]::IsPathRooted($Path)) { return $Path }
    return (Join-Path $root $Path)
}

function Get-PortOwner {
    param([int]$LocalPort)
    $found = @()
    $cmd = Get-Command Get-NetTCPConnection -ErrorAction SilentlyContinue
    if ($cmd) {
        try {
            $found = @(Get-NetTCPConnection -LocalPort $LocalPort -State Listen -ErrorAction Stop |
                Select-Object -ExpandProperty OwningProcess -Unique)
        }
        catch { $found = @() }
    }
    if (-not $cmd) {
        # 老系统没有 Get-NetTCPConnection：退回 netstat（最后一列是 PID）。
        foreach ($line in (netstat -ano | Select-String -Pattern ":$LocalPort\s")) {
            $parts = ($line.ToString() -split '\s+') | Where-Object { $_ -ne '' }
            if ($parts.Count -ge 5) { $found += [int]$parts[-1] }
        }
        $found = @($found | Select-Object -Unique)
    }
    return $found
}

function Format-Bytes {
    param([int64]$Bytes)
    if ($Bytes -ge 1048576) { return ('{0:N1} MB' -f ($Bytes / 1MB)) }
    if ($Bytes -ge 1024) { return ('{0:N1} KB' -f ($Bytes / 1KB)) }
    return "$Bytes B"
}

# /api/models 需要页面里的 token（页面契约：window.__RROCR__ = { token: "…" }）。
function Get-ModelInventory {
    param([string]$BaseUrl)
    $page = Invoke-WebRequest -Uri "$BaseUrl/" -UseBasicParsing -TimeoutSec 10
    $token = [regex]::Match($page.Content, 'token:\s*"([0-9a-f]{64})"').Groups[1].Value
    if (-not $token) {
        throw "GET $BaseUrl/ 里没有找到 token（页面契约变了？）"
    }
    return (Invoke-RestMethod -Uri "$BaseUrl/api/models" -Headers @{ 'X-RapidStructure-Token' = $token } -TimeoutSec 60)
}

# 只算**运行计划里**缺的文件（内建默认表声明的、不参与本次运行的集合不算）。
# plan[] 没有 source_url，所以按 name+role 去 sets[].files[] 里取声明。
function Get-MissingModels {
    param($Inventory, [string]$OcrDirAbs, [string]$TableModelAbs)
    $byKey = @{}
    foreach ($set in @($Inventory.sets)) {
        foreach ($f in @($set.files)) {
            $key = "$($f.name)|$($f.role)"
            if (-not $byKey.ContainsKey($key)) { $byKey[$key] = $f }
        }
    }

    $rows = @()
    foreach ($pipeline in @('text', 'formula', 'table')) {
        $pipe = $Inventory.pipelines.$pipeline
        if (-not $pipe) { continue }
        foreach ($entry in @($pipe.plan)) {
            if ($entry.state -eq 'present') { continue }

            $decl = $null
            $key = "$($entry.name)|$($entry.role)"
            if ($byKey.ContainsKey($key)) { $decl = $byKey[$key] }

            $url = $null
            $sha = $entry.declared_sha256
            $size = $null
            $destDir = $OcrDirAbs
            if ($pipeline -eq 'table') { $destDir = Split-Path -Parent $TableModelAbs }
            if ($decl) {
                if ($decl.source_url) { $url = [string]$decl.source_url }
                if (-not $sha -and $decl.sha256) { $sha = [string]$decl.sha256 }
                if ($null -ne $decl.size_bytes -and [int64]$decl.size_bytes -gt 0) { $size = [int64]$decl.size_bytes }
            }

            $rows += [pscustomobject]@{
                Pipeline   = $pipeline
                PipelineZh = $PipelineZh[$pipeline]
                Name       = [string]$entry.name
                Role       = [string]$entry.role
                State      = [string]$entry.state
                DestDir    = $destDir
                Url        = $url
                Sha256     = $sha
                SizeBytes  = $size
            }
        }
    }

    # 表格模型不属于任何集合（命令行指定）：读它旁边的 manifest.json 补 URL/哈希。
    foreach ($r in $rows) {
        if ($r.Pipeline -ne 'table' -or $r.Url -or -not $TableModelAbs) { continue }
        $mPath = Join-Path (Split-Path -Parent $TableModelAbs) 'manifest.json'
        if (-not (Test-Path -LiteralPath $mPath -PathType Leaf)) { continue }
        try {
            $m = Read-Utf8Json -Path $mPath
            foreach ($f in @($m.files)) {
                if ($f.name -eq $r.Name) {
                    if ($f.source_url) { $r.Url = [string]$f.source_url }
                    if (-not $r.Sha256 -and $f.sha256) { $r.Sha256 = [string]$f.sha256 }
                    if ($null -eq $r.SizeBytes -and $null -ne $f.size_bytes) { $r.SizeBytes = [int64]$f.size_bytes }
                }
            }
        }
        catch {
            Write-Host ("  （读 $mPath 失败：$($_.Exception.Message)）") -ForegroundColor DarkYellow
        }
    }

    return $rows
}

function Show-MissingReport {
    param($Rows)
    Write-Host ''
    Write-Host ("  缺少 {0} 个模型文件：" -f @($Rows).Count) -ForegroundColor Yellow
    foreach ($r in $Rows) {
        $size = if ($null -ne $r.SizeBytes) { Format-Bytes $r.SizeBytes } else { '大小未知' }
        Write-Host ''
        Write-Host ("    [{0}] {1}" -f $r.PipelineZh, $r.Name) -ForegroundColor Yellow
        Write-Host ("        角色     : {0}（状态 {1}，{2}）" -f $r.Role, $r.State, $size) -ForegroundColor DarkGray
        Write-Host ("        保存为   : {0}" -f (Join-Path $r.DestDir $r.Name))
        if ($r.Url) {
            Write-Host ("        下载地址 : {0}" -f $r.Url)
            $mirror = Get-MirrorUrl -Url $r.Url
            if ($mirror) {
                Write-Host ("        下载镜像 : {0}" -f $mirror) -ForegroundColor DarkGray
            }
        }
        else {
            Write-Host '        下载地址 : 清单未声明下载地址（该文件只能手动准备）' -ForegroundColor DarkYellow
        }
        if ($r.Sha256) {
            Write-Host ("        SHA-256  : {0}" -f $r.Sha256)
        }
        else {
            Write-Host '        SHA-256  : 清单未声明，下载后无法校验' -ForegroundColor DarkYellow
        }
    }
}

# 表格模型的 URL/哈希来自它**旁边**的 manifest.json：它不属于任何模型集
# （由 --table-model 命令行给出），所以 /api/models 的 plan[] 里没有 source_url。
function Get-TableModelRow {
    param([string]$TableModelAbs)
    $dir = Split-Path -Parent $TableModelAbs
    $row = [pscustomobject]@{
        Pipeline   = 'table'
        PipelineZh = $PipelineZh['table']
        Name       = Split-Path -Leaf $TableModelAbs
        Role       = 'table_structure'
        State      = 'missing'
        DestDir    = $dir
        Url        = $null
        Sha256     = $null
        SizeBytes  = $null
    }
    $mPath = Join-Path $dir 'manifest.json'
    if (-not (Test-Path -LiteralPath $mPath -PathType Leaf)) { return $row }
    try {
        $m = Read-Utf8Json -Path $mPath
        foreach ($f in @($m.files)) {
            if ($f.name -eq $row.Name) {
                if ($f.source_url) { $row.Url = [string]$f.source_url }
                if ($f.sha256) { $row.Sha256 = [string]$f.sha256 }
                if ($null -ne $f.size_bytes) { $row.SizeBytes = [int64]$f.size_bytes }
            }
        }
    }
    catch {
        Write-Host ("  （读 $mPath 失败：$($_.Exception.Message)）") -ForegroundColor DarkYellow
    }
    return $row
}

# 同意只问一次。三个输入源，按优先级：本次运行的 -Download / -NoDownload（脚本级变量，
# 菜单里的"高级选项 → 下载授权"与命令行都汇到这两个变量上）→ 非交互式一律不问 →
# 交互式问一次。授权入口只有这一个函数。
$script:ConsentState = $null
$script:RunDownload = $false
$script:RunNoDownload = $false
function Test-DownloadConsent {
    if ($null -ne $script:ConsentState) { return $script:ConsentState }
    if ($script:RunDownload) { $script:ConsentState = $true; return $true }
    if ($script:RunNoDownload) { $script:ConsentState = $false; return $false }
    # Agent / 管道 / 重定向输入：不问。（按键注入模式是自动化测试缝，允许走到询问。）
    if (-not (Test-MenuInjected)) {
        if (-not [Environment]::UserInteractive -or [Console]::IsInputRedirected) {
            $script:ConsentState = $false
            return $false
        }
    }
    # 选择题，不是填空题：第一项是推荐项。
    $items = @(
        @{ Key = 'yes'; Label = '现在下载（推荐）'; Badge = ''; Hint = '逐个校验 SHA-256；不匹配就删掉临时文件并停下，绝不换源重下'; Disabled = $false },
        @{ Key = 'no'; Label = '暂不下载'; Badge = ''; Hint = '只要上面那些下载地址，自己下'; Disabled = $false }
    )
    $sel = Show-ArrowMenu -Title '这些模型文件缺失' -Subtitle '要现在自动下载到上面列出的目录吗？' -Items $items -DefaultIndex 0
    $script:ConsentState = ($sel -eq 'yes')
    return $script:ConsentState
}

function Show-DownloadHelp {
    Write-Host ''
    Write-Host '  手动下载：把每个文件存到「保存为」给出的路径，文件名保持一致。' -ForegroundColor Yellow
    Write-Host '            清单里给了「下载地址 / 下载镜像」两个来源，任选其一即可（存哪个都一样，' -ForegroundColor Yellow
    Write-Host '            脚本会在启动后按清单声明的 SHA-256 校验）。' -ForegroundColor Yellow
    Write-Host '  自动下载：重跑并加 -Download（例如  .\serve.ps1 -Download  ；' -ForegroundColor Yellow
    Write-Host '            全功能档  .\serve.ps1 -ServeConfig models\configs\serve-local-full.json -Download）' -ForegroundColor Yellow
    Write-Host '  只检查不启动：加 -ModelsOnly' -ForegroundColor Yellow
}

function Invoke-ModelDownload {
    param($Rows, [string]$Root)
    $needsDir = @($Rows | ForEach-Object { $_.DestDir } | Select-Object -Unique)
    foreach ($dir in $needsDir) {
        if (-not (Test-Path -LiteralPath $dir -PathType Container)) {
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
            Write-Host ("  已创建目录 {0}" -f $dir) -ForegroundColor DarkGray
        }
    }

    # 空间检查：只按声明了体积的文件估算（未知体积的不算，最后仍可能失败）。
    $need = 0
    foreach ($r in $Rows) {
        if ($r.Url -and $null -ne $r.SizeBytes) { $need += $r.SizeBytes }
    }
    if ($need -gt 0) {
        $driveName = (Get-Item -LiteralPath $needsDir[0]).PSDrive.Name
        $free = (Get-PSDrive -Name $driveName).Free
        if ($free -lt ($need * 1.05)) {
            throw ("磁盘空间不足：需要约 {0}，{1}: 只剩 {2}（下完后请再跑一次）" -f (Format-Bytes $need), $driveName, (Format-Bytes $free))
        }
    }

    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue

    foreach ($r in $Rows) {
        if (-not $r.Url) {
            Write-Host ("  跳过 {0}：清单没有声明下载地址" -f $r.Name) -ForegroundColor DarkYellow
            continue
        }
        $urlHost = ''
        try { $urlHost = ([uri]$r.Url).Host } catch { $urlHost = '' }
        if ($AllowedDownloadHosts -notcontains $urlHost) {
            Write-Host ("  跳过 {0}：来源 {1} 不在允许的域名内（请手动下载）" -f $r.Name, $urlHost) -ForegroundColor DarkYellow
            continue
        }

        $dest = Join-Path $r.DestDir $r.Name
        $part = "$dest.part"
        $size = if ($null -ne $r.SizeBytes) { Format-Bytes $r.SizeBytes } else { '大小未知' }

        # 注意 @(...)：PowerShell 的函数返回值会被展开，Get-DownloadCandidates 在只有
        # 一个来源时 `return @($Url)` 到调用方就变成**字符串**，`$candidates[0]` 会取到
        # 首字符（'h'）——于是"下载"变成去请求一个叫 h 的主机。
        $candidates = @(Get-DownloadCandidates -Url $r.Url)
        if ($candidates.Count -gt 1) {
            Write-Host ("  可用来源：{0}（备用 {1}）" -f ([uri]$candidates[0]).Host, ([uri]$candidates[1]).Host) -ForegroundColor DarkGray
        }

        $saved = $false
        for ($ci = 0; $ci -lt $candidates.Count; $ci++) {
            $tryUrl = $candidates[$ci]
            $suffix = if ($ci -eq 0) { '' } else { '（镜像）' }
            Write-Host ("  下载 {0}（{1}）{2}…" -f $r.Name, $size, $suffix) -ForegroundColor Cyan

            $ok = $false
            if ($curl) {
                # curl 走磁盘流式，比 PS 5.1 的 Invoke-WebRequest 更适合几百 MB 的文件。
                # --connect-timeout / --speed-* 不是装饰：官方 HF 在国内常连不上，没有它们 curl
                # 会挂到 TCP 超时（几分钟），"失败就回退镜像"这条分支等于永远不触发。
                & curl.exe --location --fail --show-error --connect-timeout 20 --speed-limit 20480 --speed-time 30 --output $part $tryUrl
                $ok = ($LASTEXITCODE -eq 0)
            }
            else {
                try {
                    Invoke-WebRequest -Uri $tryUrl -OutFile $part -UseBasicParsing
                    $ok = $true
                }
                catch { $ok = $false }
            }

            if (-not $ok) {
                if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
                if ($ci -lt ($candidates.Count - 1)) {
                    Write-Host ("    传输失败，改试镜像 {0}" -f ([uri]$candidates[$ci + 1]).Host) -ForegroundColor DarkYellow
                    continue
                }
                throw ("下载失败：{0}" -f $tryUrl)
            }

            if ($r.Sha256) {
                $got = (Get-FileHash -LiteralPath $part -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($got -ne $r.Sha256.ToLowerInvariant()) {
                    Remove-Item -LiteralPath $part -Force
                    throw ("SHA-256 不匹配（临时文件已删除）：{0}`n      期望 {1}`n      实际 {2}" -f $r.Name, $r.Sha256, $got)
                }
                Write-Host ("    校验通过 {0}…" -f $r.Sha256.Substring(0, 16)) -ForegroundColor Green
            }
            else {
                Write-Host '    ⚠ 清单未声明 SHA-256，未校验' -ForegroundColor DarkYellow
            }

            Move-Item -LiteralPath $part -Destination $dest -Force
            Write-Host ("    已保存 {0}" -f $dest) -ForegroundColor Green
            $saved = $true
            break
        }

        if (-not $saved) { throw ("没有可用于 {0} 的下载地址" -f $r.Name) }
    }
}

function Stop-ServeChild {
    param($Process)
    if ($null -eq $Process) { return }
    if (-not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    }
}

# ============================================================================
# 第 2 层：UI 状态（只记界面偏好，不碰服务配置、不碰命令行参数）
# ============================================================================

function Get-UiStatePath {
    # 当前用户的本地应用数据目录；没有时退回 TEMP，再没有就退回仓库根（少见）。
    $base = $null
    if ($env:LOCALAPPDATA) { $base = $env:LOCALAPPDATA }
    elseif ($env:TEMP) { $base = $env:TEMP }
    else { $base = $root }
    return (Join-Path $base 'RapidStructure\serve-ui.json')
}

# 读：缺失 → 默认且不报错；损坏 / 版本不认识 / 档位标识不认识 → 默认 + 一条 Warning。
# 永远不抛异常：状态文件坏掉不能挡着服务启动。
function Get-ServeUiState {
    $path = Get-UiStatePath
    $state = [pscustomobject]@{
        Path        = $path
        LastProfile = $DefaultProfileId
        Source      = 'default'
        Warning     = $null
    }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $state }
    try {
        $json = Read-Utf8Json -Path $path
        $version = $null
        if ($null -ne $json.version) { $version = [int]$json.version }
        if ($version -ne 1) {
            $state.Source = 'corrupt'
            $state.Warning = ("UI 状态文件版本不认识（version={0}）：已忽略它，回退 {1}。文件：{2}" -f `
                    $version, $ProfileLabels[$DefaultProfileId], $path)
            return $state
        }
        $last = [string]$json.lastProfile
        if (-not $ProfileMap.Contains($last)) {
            $state.Source = 'corrupt'
            $state.Warning = ("UI 状态文件里的档位标识 '{0}' 不认识：已忽略它，回退 {1}。文件：{2}" -f `
                    $last, $ProfileLabels[$DefaultProfileId], $path)
            return $state
        }
        $state.LastProfile = $last
        $state.Source = 'file'
    }
    catch {
        # 只说人话：.NET 的 JSON 解析原文（英文、带 position）对用户没有用。
        $state.Source = 'corrupt'
        $state.Warning = ("UI 状态文件读不出来（内容不是合法 JSON）：已忽略它，回退 {0}。文件：{1}" -f `
                $ProfileLabels[$DefaultProfileId], $path)
    }
    return $state
}

# 写：先写 .tmp 再原子替换；失败只警告，不影响服务。只在**成功启动之后**调用。
function Save-ServeUiState {
    param([string]$ProfileId)
    if (-not $ProfileMap.Contains($ProfileId)) { return }
    $path = Get-UiStatePath
    try {
        $dir = Split-Path -Parent $path
        if (-not (Test-Path -LiteralPath $dir -PathType Container)) {
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
        }
        $tmp = "$path.tmp"
        $json = ([pscustomobject]@{ version = 1; lastProfile = $ProfileId } | ConvertTo-Json -Compress)
        Set-Content -LiteralPath $tmp -Value $json -Encoding ASCII
        Move-Item -LiteralPath $tmp -Destination $path -Force
    }
    catch {
        Write-Host ("  ⚠ 没能记住这次的档位（{0}）；不影响服务运行。" -f $_.Exception.Message) -ForegroundColor DarkYellow
    }
}

# ============================================================================
# 第 2 层：交互（方向键菜单，不含任何业务）
# ============================================================================

# 终端能不能安全读方向键？不能就不进菜单：Agent / 管道 / 重定向一律不等待按键。
function Test-InteractiveConsole {
    if (-not [Environment]::UserInteractive) { return $false }
    try {
        if ([Console]::IsInputRedirected) { return $false }
        if ([Console]::IsOutputRedirected) { return $false }
    }
    catch { return $false }
    return $true
}

# 测试缝：只有显式 -Menu 且设置了环境变量时才装载注入队列；普通调用（含无参数调用）
# 永远读真实键盘。队列耗尽即视为 Esc / 用默认值，绝不阻塞。
$script:MenuKeyQueue = $null
$script:MenuValueQueue = $null
function Initialize-MenuInput {
    if (-not $Menu) { return }
    if ($env:RAPID_STRUCTURE_SERVE_MENU_KEYS) {
        $script:MenuKeyQueue = [System.Collections.Queue]::new()
        foreach ($k in ($env:RAPID_STRUCTURE_SERVE_MENU_KEYS -split ',')) {
            if ($k.Trim()) { $script:MenuKeyQueue.Enqueue($k.Trim()) }
        }
    }
    if ($env:RAPID_STRUCTURE_SERVE_MENU_VALUES) {
        $script:MenuValueQueue = [System.Collections.Queue]::new()
        foreach ($v in ($env:RAPID_STRUCTURE_SERVE_MENU_VALUES -split '\|')) { $script:MenuValueQueue.Enqueue($v) }
    }
}

function Test-MenuInjected {
    return (($null -ne $script:MenuKeyQueue) -or ($null -ne $script:MenuValueQueue))
}

# 按键 → 'Up' / 'Down' / 'Home' / 'End' / 'Enter' / 'Escape'，其它键返回字符本身。
function Read-MenuKey {
    if ($null -ne $script:MenuKeyQueue) {
        if ($script:MenuKeyQueue.Count -eq 0) {
            Write-Host '  [注入按键] 队列已空 → 视为 Esc' -ForegroundColor DarkYellow
            return 'Escape'
        }
        $k = [string]$script:MenuKeyQueue.Dequeue()
        Write-Host ("  [注入按键] {0}" -f $k) -ForegroundColor DarkYellow
        return $k
    }
    $ki = [Console]::ReadKey($true)
    switch ($ki.Key) {
        'UpArrow'   { return 'Up' }
        'DownArrow' { return 'Down' }
        'Home'      { return 'Home' }
        'End'       { return 'End' }
        'Enter'     { return 'Enter' }
        'Escape'    { return 'Escape' }
    }
    return [string]$ki.KeyChar
}

# 唯一的填空题入口：只有"高级选项 → 端口 → 自定义端口…"会用到它。其它决定一律
# 走 Show-ArrowMenu 的选择题（推荐项默认选中），不要让用户手打值。
function Read-MenuValue {
    param([string]$Prompt, [string]$Default = '')
    if ($null -ne $script:MenuValueQueue) {
        if ($script:MenuValueQueue.Count -eq 0) {
            Write-Host '  [注入输入] 队列已空 → 用默认值' -ForegroundColor DarkYellow
            return $Default
        }
        $v = [string]$script:MenuValueQueue.Dequeue()
        Write-Host ("  [注入输入] {0} = {1}" -f $Prompt, $v) -ForegroundColor DarkYellow
        return $v
    }
    $shown = if ($Default) { "$Prompt [$Default]" } else { $Prompt }
    $answer = Read-Host "  $shown"
    if ([string]::IsNullOrWhiteSpace($answer)) { return $Default }
    return $answer.Trim()
}

# 上下移动跳过不可用项（禁用项仍看得见，也读得到理由，只是不能确认）。
function Move-MenuIndex {
    param([array]$Items, [int]$Index, [string]$Direction)
    $n = $Items.Count
    $i = $Index
    for ($step = 1; $step -le $n; $step++) {
        if ($Direction -eq 'up') { $i = $i - 1; if ($i -lt 0) { $i = $n - 1 } }
        elseif ($Direction -eq 'down') { $i = $i + 1; if ($i -ge $n) { $i = 0 } }
        elseif ($Direction -eq 'home') { $i = $step - 1 }
        else { $i = $n - $step }
        if (-not [bool]$Items[$i].Disabled) { return $i }
    }
    return $Index
}

# 能不能用光标定位"就地重画"（demo.ps1 的无闪烁做法）。输出被重定向
# （脚本 / Agent / 管道）时 SetCursorPosition 会抛，这时退回顺序打印。
# 测试缝：RAPID_STRUCTURE_SERVE_MENU_INPLACE 非空时假装可用 —— Agent 环境里没有真终端，
# "原地重画 + 失败即降级"这条分支否则永远测不到（它正是用户报的那次崩溃的兜底）。
function Test-ConsoleCursor {
    if ($env:RAPID_STRUCTURE_SERVE_MENU_INPLACE) { return $true }
    try {
        [Console]::SetCursorPosition(0, [Console]::CursorTop)
        return $true
    }
    catch { return $false }
}

# 读光标的当前行。任何控制台 API 在句柄无效时都会抛，读不到就返回 -1。
# 读光线（Clear-MenuScreen + Get-MenuCursorTop + Set-MenuCursor）一起，
# 保证"控制台不听话"永远只导致降级，不会把 .NET 的英文异常糊在用户屏幕上。
function Get-MenuCursorTop {
    if ($env:RAPID_STRUCTURE_SERVE_MENU_INPLACE) { return 8 }
    try { return [int][Console]::CursorTop } catch { return -1 }
}

# 把光标移到指定行；失败返回 $false。
# 所有原地重画都必须走这里：窗口被缩小、或缓冲区滚走之后，先前记下的行号会越界，
# SetCursorPosition 会抛异常并把 .NET 的长篇英文糊在用户屏幕上（实测 top=78 越界）。
# 这个函数**故意不认测试缝**：降级路径就是由它失败触发的。
function Set-MenuCursor {
    param([int]$Row)
    try { [Console]::SetCursorPosition(0, $Row); return $true }
    catch { return $false }
}

# 终端按"显示列"排版：汉字占两列，而 PadRight 按**字符数**补空格。中英混排的行用 PadRight
# 补到窗口宽度就会被撑出窗口 → 软换行 → 就地重画的行号全部错位（用户实测到的"选项挤成一行"
# 就是这么来的）。这里只覆盖菜单里真正会出现的范围（CJK / 全角），不搬完整的 East Asian Width 表。
function Get-DisplayWidth {
    param([string]$Text)
    if (-not $Text) { return 0 }
    $w = 0
    foreach ($ch in $Text.ToCharArray()) {
        $c = [int]$ch
        if (($c -ge 0x1100 -and $c -le 0x115F) -or ($c -ge 0x2E80 -and $c -le 0xA4CF) -or
            ($c -ge 0xAC00 -and $c -le 0xD7A3) -or ($c -ge 0xF900 -and $c -le 0xFAFF) -or
            ($c -ge 0xFE30 -and $c -le 0xFE6F) -or ($c -ge 0xFF00 -and $c -le 0xFF60) -or
            ($c -ge 0xFFE0 -and $c -le 0xFFE6)) { $w += 2 }
        else { $w += 1 }
    }
    return $w
}

# 就地重画时把一行补空格擦到窗口宽度以内。$UsedColumns 是这一行已经写掉的列数（标记前缀）。
# 留 3 列余量：写满最后一列本身就会触发软换行，行号一错位菜单就花了。
function Add-MenuPad {
    param([string]$Text, [int]$UsedColumns = 0, [int]$WindowWidth = 0)
    if ($WindowWidth -le 0) { return $Text }
    $pad = ($WindowWidth - 3) - $UsedColumns - (Get-DisplayWidth $Text)
    if ($pad -le 0) { return $Text }
    return $Text + (' ' * $pad)
}

# 菜单界面开头清屏。两个作用：
#   ① 上一屏的文字（模型报告、诊断、上一次菜单）不与新菜单堆在一起 —— 用户按 Enter 之后屏幕就干净；
#   ② 选项区永远从屏幕上方开始，不会因为缓冲区滚动让先前记下的行号失效。
# 只在真正的交互终端里清；重定向（Agent 测试 / 写日志）时什么都不做，保证日志可读。
function Clear-MenuScreen {
    if ($env:RAPID_STRUCTURE_SERVE_MENU_INPLACE) { return $true }   # 假装清过屏：日志要留给人看
    if (-not (Test-ConsoleCursor)) { return $false }
    try {
        [Console]::Clear()
        [Console]::SetCursorPosition(0, 0) | Out-Null
        return $true
    }
    catch { return $false }
}

# 通用方向键菜单：返回选中项的 Key；Esc（或没有可选项）返回 $null。不含任何业务。
# 渲染照 demo.ps1：选项区在原地重画（只重画选项行，所以不闪），改动前后：
#   进入时清屏一次（见 Clear-MenuScreen），选项行随后逐帧就地重画。
# 选中 ● 绿色、未选中 ○ 灰色、禁用项暗色但仍看得见。
function Show-ArrowMenu {
    param(
        [string]$Title,
        [string]$Subtitle,
        [array]$Items,
        [int]$DefaultIndex = 0
    )
    if (-not $Items -or $Items.Count -eq 0) { return $null }
    # 用户最后停在哪一行（$script:MenuLastIndex）：调用方可以把它当下次的 -DefaultIndex，
    # 这样"按 Enter 做完一件事再按 Enter"打的是同一行，而不是跳回第一行。
    if ($null -eq $script:MenuLastIndex) { $script:MenuLastIndex = 0 }
    $n = $Items.Count
    $idx = [Math]::Max(0, [Math]::Min($DefaultIndex, $n - 1))

    # 每个菜单都从干净的屏幕开始（按 Enter 之后不再堆叠上一屏）。
    $inPlace = Clear-MenuScreen

    $lineCount = 0
    $maxColumns = 0
    foreach ($it in $Items) {
        $lineCount += $(if ($it.Hint) { 2 } else { 1 })
        $w = 4 + (Get-DisplayWidth ([string]$it.Label))
        if ($it.Badge) { $w += 4 + (Get-DisplayWidth ([string]$it.Badge)) }
        if ($it.Hint) { $w = [Math]::Max($w, 6 + (Get-DisplayWidth ([string]$it.Hint))) }
        $maxColumns = [Math]::Max($maxColumns, $w)
    }
    # 就地重画的前提：整块选项区在窗口里，且没有一行会被软换行撑破（撑破就整块错位，
    # 用户看到的就是"选项挤成一行"）。任一不满足都退回顺序打印（宁可啰嗦，不要花屏）。
    $windowWidth = 0
    if ($inPlace) {
        $rows = 0
        try { $rows = [Console]::WindowHeight } catch { $rows = 0 }
        try { $windowWidth = [int][Console]::WindowWidth } catch { $windowWidth = 0 }
        if ($windowWidth -le 0) { $inPlace = $false }
        elseif ($rows -gt 0 -and ($lineCount + 5) -gt $rows) { $inPlace = $false }
        elseif (($maxColumns + 1) -gt ($windowWidth - 3)) { $inPlace = $false }
    }

    $cursorWasVisible = $true
    if ($inPlace) {
        try { $cursorWasVisible = [Console]::CursorVisible; [Console]::CursorVisible = $false } catch { }
    }

    $optionTop = 0
    $afterBottom = 0
    if ($inPlace) {
        Write-Host ''
        Write-Host ("  {0}" -f $Title) -ForegroundColor Cyan
        if ($Subtitle) { Write-Host ("  {0}" -f $Subtitle) -ForegroundColor DarkGray }
        Write-Host ''
        $optionTop = Get-MenuCursorTop
        $bufferHeight = 0
        try { $bufferHeight = [Console]::BufferHeight } catch { $bufferHeight = 0 }
        if ($optionTop -lt 0) {
            # 光标位置都读不到（句柄无效）：顺序打印，不许抛异常。
            $inPlace = $false
        }
        elseif ($bufferHeight -gt 0 -and ($optionTop + $lineCount + 1) -ge $bufferHeight) {
            # 预留不出空间（屏幕已经贴着缓冲区底部）：顺序打印，光标不再乱跳。
            $inPlace = $false
        }
        else {
            for ($i = 0; $i -lt $lineCount; $i++) { Write-Host '' }
            Write-Host '  ↑↓ 移动 · Enter 确认 · Esc 返回 · Home/End 首尾' -ForegroundColor DarkGray
            $afterBottom = Get-MenuCursorTop
            if ($afterBottom -lt 0) { $inPlace = $false }
        }
    }

    $paint = {
        param([bool]$inPlace)
        # 返回 $false 表示"原地绘制失败"，调用方退回顺序打印。
        if (-not $inPlace) {
            # 顺序模式：每帧都把标题一起重打，日志（重定向）里读起来才完整。
            Write-Host ''
            Write-Host ("  {0}" -f $Title) -ForegroundColor Cyan
            if ($Subtitle) { Write-Host ("  {0}" -f $Subtitle) -ForegroundColor DarkGray }
            Write-Host ''
        }
        $row = $optionTop
        for ($i = 0; $i -lt $n; $i++) {
            $it = $Items[$i]
            $badge = if ($it.Badge) { "    $($it.Badge)" } else { '' }
            $label = "{0}{1}" -f $it.Label, $badge
            if ($inPlace -and -not (Set-MenuCursor -Row $row)) { return $false }
            # 标记前缀已经占掉 4 列，补空格时必须把它算进去（否则整行仍会超出窗口 → 软换行）。
            $body = if ($inPlace) { Add-MenuPad -Text $label -UsedColumns 4 -WindowWidth $windowWidth } else { $label }
            if ($i -eq $idx) {
                Write-Host '  ● ' -NoNewline -ForegroundColor Green
                Write-Host $body -ForegroundColor Green
            }
            elseif ([bool]$it.Disabled) {
                Write-Host '  ○ ' -NoNewline -ForegroundColor DarkGray
                Write-Host $body -ForegroundColor DarkGray
            }
            else {
                Write-Host '  ○ ' -NoNewline -ForegroundColor DarkGray
                Write-Host $body -ForegroundColor Gray
            }
            $row++
            if ($it.Hint) {
                if ($inPlace -and -not (Set-MenuCursor -Row $row)) { return $false }
                $hint = "      {0}" -f $it.Hint
                Write-Host $(if ($inPlace) { Add-MenuPad -Text $hint -WindowWidth $windowWidth } else { $hint }) -ForegroundColor DarkGray
                $row++
            }
        }
        if ($inPlace) {
            if (-not (Set-MenuCursor -Row $afterBottom)) { return $false }
        }
        else {
            Write-Host ''
            Write-Host '  ↑↓ 移动 · Enter 确认 · Esc 返回 · Home/End 首尾' -ForegroundColor DarkGray
        }
        return $true
    }

    try {
        if ($inPlace) {
            if (-not (& $paint $true)) { $inPlace = $false; [void](& $paint $false) }
        }
        else { [void](& $paint $false) }
        while ($true) {
            $key = Read-MenuKey
            switch ($key) {
                'Up'   { $idx = Move-MenuIndex -Items $Items -Index $idx -Direction 'up' }
                'Down' { $idx = Move-MenuIndex -Items $Items -Index $idx -Direction 'down' }
                'Home' { $idx = Move-MenuIndex -Items $Items -Index $idx -Direction 'home' }
                'End'  { $idx = Move-MenuIndex -Items $Items -Index $idx -Direction 'end' }
                'Enter' {
                    if ([bool]$Items[$idx].Disabled) {
                        # 禁用项也能确认，但只回一句"为什么不行"，不执行、不退出。
                        if ($inPlace -and -not (Set-MenuCursor -Row $afterBottom)) { $inPlace = $false }
                        Write-Host ''
                        Write-Host '  （这一项当前不可用，理由见上面那行）' -ForegroundColor DarkYellow
                        if ($inPlace) { Start-Sleep -Milliseconds 900 }
                    }
                    else {
                        if ($inPlace) { [void](Set-MenuCursor -Row $afterBottom) }
                        # 让调用方知道用户最后停在哪一行：菜单回来时把光标放回原处
                        # （否则每次动作后光标跳回第一行，下一次 Enter 就按到别的项上）。
                        $script:MenuLastIndex = $idx
                        return $Items[$idx].Key
                    }
                }
                'Escape' {
                    if ($inPlace) { [void](Set-MenuCursor -Row $afterBottom) }
                    $script:MenuLastIndex = $idx
                    return $null
                }
            }
            if ($inPlace) {
                if (-not (& $paint $true)) { $inPlace = $false; [void](& $paint $false) }
            }
            else { [void](& $paint $false) }
        }
    }
    finally {
        if ($inPlace -and $cursorWasVisible) { try { [Console]::CursorVisible = $true } catch { } }
    }
}

function Wait-MenuContinue {
    param([string]$Message = '按 Enter 返回主菜单')
    Write-Host ''
    Write-Host ("  {0}" -f $Message) -ForegroundColor DarkGray
    $null = Read-MenuKey
    # 读完键就清屏：下一屏（菜单或报告）从干净的屏幕开始，不堆叠。
    [void](Clear-MenuScreen)
}

# 确认也做成选择题（第一项就是推荐项），不让用户记"该按哪个键"。
function Wait-MenuConfirm {
    param(
        [string]$YesLabel = '开始（推荐）',
        [string]$NoLabel = '返回，什么都不做'
    )
    $items = @(
        @{ Key = 'yes'; Label = $YesLabel; Badge = ''; Hint = $null; Disabled = $false },
        @{ Key = 'no'; Label = $NoLabel; Badge = ''; Hint = $null; Disabled = $false }
    )
    $sel = Show-ArrowMenu -Title '确认' -Subtitle '上面那份计划就是要执行的内容。' -Items $items -DefaultIndex 0
    return ($sel -eq 'yes')
}

# ============================================================================
# 第 3 层：业务逻辑（原有流程原样搬进函数；入口只决定传什么进来）
# ============================================================================

# 生效值：命令行显式参数 > 菜单选择（两者都已经在 Plan 里）> profile > 内建默认。
# 菜单的选择在合并时就写进了 Plan，所以这里仍然只有**一条**优先级链。
function Resolve-ServePlan {
    param([hashtable]$Plan)

    # ==== profile ====
    if ($Plan.ServeConfig) { $profilePath = Resolve-RepoPath $Plan.ServeConfig }
    else { $profilePath = Resolve-RepoPath $ProfileMap[$DefaultProfileId] }
    $cfg = $null
    if (Test-Path -LiteralPath $profilePath -PathType Leaf) {
        $cfg = Read-Utf8Json -Path $profilePath
    }
    elseif ($Plan.ServeConfig) {
        # 显式指定就不许静默退回别的档位：把实际路径和原因一起抛出去。
        throw "profile not found: $profilePath"
    }
    else {
        Write-Host "serve: profile not found, falling back to built-in defaults ($profilePath)" -ForegroundColor DarkYellow
    }

    # ==== 生效值 ====
    $portValue = if ($null -ne $Plan.Port) { [int]$Plan.Port }
                 elseif ($cfg -and $null -ne $cfg.port) { [int]$cfg.port }
                 else { 8760 }

    $modelDirValue = if ($Plan.ModelDir) { $Plan.ModelDir }
                     elseif ($cfg -and $cfg.model_dir) { [string]$cfg.model_dir }
                     else { 'models\text\small' }

    $configValue = if ($Plan.Config) { $Plan.Config }
                   elseif ($cfg -and $cfg.config) { [string]$cfg.config }
                   else { 'models\configs\test-config-small.yaml' }

    $tableValue = if ($Plan.TableModel) { $Plan.TableModel }
                  elseif ($cfg -and $cfg.table_model) { [string]$cfg.table_model }
                  else { 'models\table\slanet-plus.onnx' }

    $tableWanted = $true
    if ($null -ne $Plan.TableEnabled) { $tableWanted = [bool]$Plan.TableEnabled }
    elseif ($cfg -and $null -ne $cfg.table_enabled) { $tableWanted = [bool]$cfg.table_enabled }

    $openValue = $true
    if ($null -ne $Plan.Open) { $openValue = [bool]$Plan.Open }
    elseif ($cfg -and $null -ne $cfg.open) { $openValue = [bool]$cfg.open }

    $providerValue = if ($Plan.Provider) { $Plan.Provider }
                     elseif ($cfg -and $cfg.provider) { [string]$cfg.provider }
                     else { $null }

    # ==== 展开成绝对路径 ====
    $modelDirAbs = Resolve-RepoPath $modelDirValue
    $configAbs = Resolve-RepoPath $configValue
    $tableAbs = if ($tableWanted) { Resolve-RepoPath $tableValue } else { $null }
    $manifestAbs = Join-Path $modelDirAbs 'manifest.json'

    # config 硬要求（推理参数错了没意义）；模型文件**不**在这里判死：缺了就交给
    # 下面的模型检查去打印下载地址，而不是直接拒绝启动。
    if (-not (Test-Path -LiteralPath $configAbs -PathType Leaf)) {
        throw "serve: --config 不存在：$configAbs（改 profile 或用 -Config 覆盖）"
    }
    $manifestFound = Test-Path -LiteralPath $manifestAbs -PathType Leaf

    # ==== 拼命令（不新增任何启动语义） ====
    $serveArgs = @('serve', '--port', "$portValue", '--model-dir', $modelDirAbs, '--config', $configAbs)
    if ($tableAbs) { $serveArgs += @('--table-model', $tableAbs) }
    if ($providerValue) { $serveArgs += @('--provider', $providerValue) }
    if ($openValue) { $serveArgs += '--open' }
    if ($Plan.Reverify) { $serveArgs += '--reverify-models' }
    if ($Plan.Rest) { $serveArgs += $Plan.Rest }

    $commandLine = '"{0}" {1}' -f $exe, ($serveArgs -join ' ')

    return [pscustomobject]@{
        ProfilePath   = $profilePath
        Config        = $cfg
        Port          = $portValue
        ModelDirAbs   = $modelDirAbs
        ConfigAbs     = $configAbs
        TableAbs      = $tableAbs
        TableWanted   = $tableWanted
        Open          = $openValue
        Provider      = $providerValue
        ManifestAbs   = $manifestAbs
        ManifestFound = $manifestFound
        ServeArgs     = $serveArgs
        CommandLine   = $commandLine
    }
}

# 启动摘要：菜单的「启动确认」与真正的启动路径共用同一段输出，避免两处漂移。
function Show-ServePlanBanner {
    param($Resolved, [string]$ModeTag)
    Write-Host ''
    Write-Host ("  本地网页服务 {0}" -f $Resolved.Port) -ForegroundColor Cyan
    Write-Host ("    profile    : {0}" -f $(if ($Resolved.Config) { $Resolved.ProfilePath } else { '(未使用，走内建默认)' })) -ForegroundColor DarkGray
    Write-Host ("    model-dir  : {0}" -f $Resolved.ModelDirAbs) -ForegroundColor DarkGray
    Write-Host ("    模型清单   : {0}" -f $(if ($Resolved.ManifestFound) { $Resolved.ManifestAbs } else { "(无 manifest.json，用内建默认模型表：文件名按 config 的 model_type 推导)" })) -ForegroundColor DarkGray
    Write-Host ("    config     : {0}" -f $Resolved.ConfigAbs) -ForegroundColor DarkGray
    Write-Host ("    table      : {0}" -f $(if ($Resolved.TableAbs) { $Resolved.TableAbs } else { '(未启用)' })) -ForegroundColor DarkGray
    Write-Host ("    provider   : {0}" -f $(if ($Resolved.Provider) { $Resolved.Provider } else { '(跟随 config)' })) -ForegroundColor DarkGray
    Write-Host ("    地址       : http://127.0.0.1:{0}/" -f $Resolved.Port) -ForegroundColor Green
    if ($ModeTag) { Write-Host ("    本次动作   : {0}" -f $ModeTag) -ForegroundColor DarkYellow }
    Write-Host ''
    Write-Host ("  {0}" -f $Resolved.CommandLine) -ForegroundColor DarkGray
    Write-Host ''
}

# 第 4 层：执行与生命周期。返回 [pscustomobject]@{ ExitCode; Started; Mode }：
#   Started = $true 表示服务**就绪过**（模型检查通过），菜单据此决定要不要记住档位。
#   硬错误（profile/config 不存在、构建失败、就绪超时、下载失败）仍然 throw，
#   由调用方决定是退出还是回到菜单。
function Invoke-ServeRun {
    param(
        [hashtable]$Plan,
        [ValidateSet('start', 'models-only')]
        [string]$Mode = 'start',
        [bool]$ShowBanner = $true,
        # 菜单路径传入选中的档位标识：服务就绪后记进状态文件。命令行路径留空（不记）。
        [string]$ProfileId = ''
    )

    # 下载授权仍然只有 Test-DownloadConsent 一个入口：这里把本次运行的
    # -Download / -NoDownload 交给它，并清掉上一次运行留下的结论缓存。
    $script:RunDownload = [bool]$Plan.Download
    $script:RunNoDownload = [bool]$Plan.NoDownload
    $script:ConsentState = $null

    $resolved = Resolve-ServePlan -Plan $Plan
    $portValue = $resolved.Port

    if ($ShowBanner) { Show-ServePlanBanner -Resolved $resolved }

    if ($Plan.DryRun) {
        Write-Host '  （-DryRun：不构建、不启动；上面那一行可以原样手敲。）' -ForegroundColor DarkGray
        return [pscustomobject]@{ ExitCode = 0; Started = $false; Mode = $Mode }
    }

    # ==== 构建（serve 不在 default feature 里） ====
    if (-not $Plan.NoBuild) {
        Write-Host '  构建 target\release\rapidstruct.exe（--features serve）…' -ForegroundColor DarkGray
        Push-Location $root
        try {
            & cargo build --release --bin rapidstruct --features serve
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build 退出码 $LASTEXITCODE"
            }
        }
        finally {
            Pop-Location
        }
    }

    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "serve: 找不到 $exe（用 -NoBuild 时请先自己构建一次）"
    }

    $modelDirAbs = $resolved.ModelDirAbs
    $tableAbs = $resolved.TableAbs
    $serveArgs = $resolved.ServeArgs
    if ($Mode -eq 'models-only') {
        # 只做模型检查时不要顺手弹一个浏览器窗口：服务起来只是为了读一次 /api/models，
        # 读完就停。用户是来做检查的，不是来看页面的。
        $serveArgs = @($serveArgs | Where-Object { $_ -ne '--open' })
    }

    # ==== 启动前必须先修的一处：--table-model 的路径在启动期就要存在 ====
    # serve 故意在启动时拒绝一个不存在的 --table-model（run.rs 的原文：「a wrong path must
    # fail at startup instead of on the first table request」），所以这一个文件不能等服务
    # 起来再靠 /api/models 发现。其余文件（OCR / 公式）由下面的服务负责回答——
    # 它们缺失时 serve 照样启动，只是 engine state = blocked_models_missing。
    if ($tableAbs -and -not (Test-Path -LiteralPath $tableAbs -PathType Leaf)) {
        $tableRow = Get-TableModelRow -TableModelAbs $tableAbs
        Show-MissingReport -Rows @($tableRow)
        Write-Host ''
        Write-Host '  注：表格模型是启动前提（serve 在这个路径不存在时直接拒绝启动，不等第一个表格请求）。' -ForegroundColor DarkYellow
        if (Test-DownloadConsent) {
            Write-Host ''
            Invoke-ModelDownload -Rows @($tableRow) -Root $root
        }
        else {
            Show-DownloadHelp
        }
        if (-not (Test-Path -LiteralPath $tableAbs -PathType Leaf)) {
            Write-Host ''
            if (Test-DownloadConsent) {
                Write-Host ("  表格模型仍然缺失：{0}（没拿到文件，或来源不在允许的域名内）" -f $tableAbs) -ForegroundColor Yellow
            }
            else {
                Write-Host ("  表格模型仍然缺失：{0}" -f $tableAbs) -ForegroundColor Yellow
            }
            return [pscustomobject]@{ ExitCode = 1; Started = $false; Mode = $Mode }
        }
        Write-Host '  表格模型已就位。' -ForegroundColor Green
    }

    # ==== 端口占用：serve 自己不换端口，这里提前说清楚是谁占着 ====
    # @(...)：只有一个占用者时 Get-PortOwner 返回的是单个 PID（int），包成数组才好 .Count。
    $owners = @(Get-PortOwner -LocalPort $portValue)
    if ($owners.Count -gt 0) {
        $described = foreach ($owner in $owners) {
            $p = Get-Process -Id $owner -ErrorAction SilentlyContinue
            if ($p) { "$($p.ProcessName) (PID $owner)" } else { "PID $owner" }
        }
        Write-Host ("  端口 {0} 已被占用：{1}" -f $portValue, ($described -join ', ')) -ForegroundColor Yellow
        Write-Host ("  确认要停掉它：Stop-Process -Id {0}" -f ($owners -join ',')) -ForegroundColor Yellow
        Write-Host '  上一次没退出的 `rapidstruct serve` 会同时锁住 target\release\deps\rapidstruct.exe，' -ForegroundColor DarkGray
        Write-Host '  让下一次 cargo build 报 LNK1104；先停掉它再重跑本脚本。' -ForegroundColor DarkGray
        return [pscustomobject]@{ ExitCode = 1; Started = $false; Mode = $Mode }
    }

    # ==== 后台启动一次，等它就绪（模型检查与正式服务共用这一个进程） ====
    $argString = ($serveArgs | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join ' '
    Write-Host '  启动服务（后台，稍后转前台；Ctrl+C 即停止）…' -ForegroundColor DarkGray
    Push-Location $root
    try {
        $proc = Start-Process -FilePath $exe -ArgumentList $argString -PassThru -NoNewWindow
    }
    finally {
        Pop-Location
    }

    $baseUrl = "http://127.0.0.1:$portValue"
    $deadline = (Get-Date).AddSeconds($Plan.ReadyTimeoutSec)
    $ready = $false
    try {
        while (-not $ready) {
            if ($proc.HasExited) {
                throw (("rapidstruct serve 提前退出（退出码 {0}）。先看它自己打印的那几行。" +
                        "缺 --table-model / --formula-detector 这类**启动期**路径时，serve 会直接拒绝启动；" +
                        "缺 OCR / 公式模型文件时它照样启动，用 -Download 补齐即可。") -f $proc.ExitCode)
            }
            try {
                $null = Invoke-WebRequest -Uri "$baseUrl/" -UseBasicParsing -TimeoutSec 3
                $ready = $true
            }
            catch {
                if ((Get-Date) -gt $deadline) {
                    throw "等待 $baseUrl 就绪超时（$($Plan.ReadyTimeoutSec) s）"
                }
                Start-Sleep -Milliseconds 250
            }
        }
    }
    catch {
        Stop-ServeChild -Process $proc
        throw
    }

    # ==== 模型盘点：/api/models 是唯一权威（三种模型在这里一并回答） ====
    # 服务已经起来了，这一段里任何异常都必须先收掉子进程再往上抛：孤儿 rapidstruct 会占着
    # 端口，还会锁住 target\release\deps\rapidstruct.exe，让下一次 cargo build 报 LNK1104。
    try { $inventory = Get-ModelInventory -BaseUrl $baseUrl }
    catch { Stop-ServeChild -Process $proc; throw }
    $missingRows = @(Get-MissingModels -Inventory $inventory -OcrDirAbs $modelDirAbs -TableModelAbs $tableAbs)

    # 注意：/api/models **顶层**的 complete/missing/corrupt 只回答**文本管线**
    # （`self.text_blocking()`），公式与表格各有自己的块。所以在顶层读它们会印出
    # "complete=True 却缺文件"这种看起来自相矛盾的结论——这里按 pipelines.* 逐条打印。
    $pipeSummary = @('text', 'formula', 'table') | ForEach-Object {
        $pipe = $inventory.pipelines.$_
        if (-not $pipe) { return $null }
        $bad = @($pipe.missing).Count + @($pipe.corrupt).Count
        $suffix = if ($bad -gt 0) { '（缺 {0}）' -f $bad } else { '' }
        '{0} {1}{2}' -f $PipelineZh[$_], $(if ($pipe.complete) { 'complete' } else { 'incomplete' }), $suffix
    }
    Write-Host ("  模型状态：source={0}；{1}；downloads_allowed={2}" -f `
            $inventory.source, ($pipeSummary -join ' · '), $inventory.downloads_allowed) -ForegroundColor DarkGray

    if ($missingRows.Count -gt 0) {
        Show-MissingReport -Rows $missingRows

        if (-not (Test-DownloadConsent)) {
            Show-DownloadHelp
            Write-Host ''
            Write-Host '  提示：这些文件缺失时服务仍能启动，但 engine state 会是 blocked_models_missing。' -ForegroundColor DarkGray
            Stop-ServeChild -Process $proc
            return [pscustomobject]@{ ExitCode = 1; Started = $false; Mode = $Mode }
        }

        Write-Host ''
        # 下载失败（传输失败、SHA-256 不匹配）会 throw：同样不能把子进程留在那里。
        try { Invoke-ModelDownload -Rows $missingRows -Root $root }
        catch { Stop-ServeChild -Process $proc; throw }

        # 不调 POST /api/models/reverify：逐文件状态是**每次请求重算**的（identity 缓存只缓存
        # "已存在且已校验"的文件，"缺失"不进缓存），所以下载完直接 GET 就能看到 present。
        # 而且 reverify 会把全部文件重新哈希一遍（全功能档含 594 MB 的公式模型），没必要。
        try {
            $inventory = Get-ModelInventory -BaseUrl $baseUrl
            $missingRows = @(Get-MissingModels -Inventory $inventory -OcrDirAbs $modelDirAbs -TableModelAbs $tableAbs)
        }
        catch { Stop-ServeChild -Process $proc; throw }
        if ($missingRows.Count -gt 0) {
            Show-MissingReport -Rows $missingRows
            Write-Host ''
            Write-Host '  仍有文件缺失（上面列出的就是还缺的）。' -ForegroundColor Yellow
            Stop-ServeChild -Process $proc
            return [pscustomobject]@{ ExitCode = 1; Started = $false; Mode = $Mode }
        }
        Write-Host '  模型已齐备。' -ForegroundColor Green
    }
    else {
        Write-Host '  模型已齐备（运行计划里的文件都在）。' -ForegroundColor Green
    }

    if ($Mode -eq 'models-only') {
        Stop-ServeChild -Process $proc
        Write-Host ''
        Write-Host '  模型检查完成（未留在前台服务）。' -ForegroundColor Green
        return [pscustomobject]@{ ExitCode = 0; Started = $true; Mode = $Mode }
    }

    # 走到这里才叫"启动成功"：服务已就绪、运行计划里的模型也都齐、没有因为缺件提前退出。
    # 记在这一刻而不是等前台结束——Ctrl+C 停服务时本脚本会随之中断，写在最后就永远写不上。
    if ($ProfileId) { Save-ServeUiState -ProfileId $ProfileId }

    # ==== 转前台：Ctrl+C 就是停止服务 ====
    Write-Host ''
    try {
        Wait-Process -Id $proc.Id -ErrorAction SilentlyContinue
    }
    finally {
        Stop-ServeChild -Process $proc
    }
    $finalCode = 0
    if ($proc.HasExited) { $finalCode = $proc.ExitCode }
    return [pscustomobject]@{ ExitCode = $finalCode; Started = $true; Mode = $Mode }
}

# ============================================================================
# 第 2 层：交互流程（主菜单 / 启动向导 / 高级选项 / 诊断）
# 这里只做选择与呈现：下载、校验、启动一律调第 3、4 层的既有函数，不复制一份。
# ============================================================================

# 合并：菜单选择叠在命令行之上，再让命令行显式值盖回来（命令行永远最优先）。
# CliPlan 里"没给"的项是 $null，所以它不会把菜单的选择冲掉。
function Merge-ServePlan {
    param([hashtable]$CliPlan, [hashtable]$Overrides)
    $plan = @{}
    foreach ($k in $CliPlan.Keys) { $plan[$k] = $CliPlan[$k] }
    if ($Overrides) { foreach ($k in $Overrides.Keys) { $plan[$k] = $Overrides[$k] } }
    foreach ($k in $CliPlan.Keys) {
        if ($null -ne $CliPlan[$k]) { $plan[$k] = $CliPlan[$k] }
    }
    return $plan
}

# 三态循环用的索引查找（显式区分 $null 与 $false，不靠 -eq 的隐式转换）。
function Get-MenuStateIndex {
    param([array]$Values, $Current)
    for ($i = 0; $i -lt $Values.Count; $i++) {
        if ($null -eq $Values[$i] -and $null -eq $Current) { return $i }
        if ($null -ne $Values[$i] -and $null -ne $Current -and $Values[$i] -eq $Current) { return $i }
    }
    return 0
}

function Select-ServeProfile {
    param([hashtable]$Session, [string]$Reason)
    if ($Session.LockedProfile) {
        # 命令行给了 -ServeConfig：不问档位，也不改记忆里的档位。
        return [pscustomobject]@{ Id = $null; Path = $Session.LockedProfile; Locked = $true }
    }
    $items = @()
    $defaultIndex = 0
    $i = 0
    foreach ($id in $ProfileMap.Keys) {
        $abs = Resolve-RepoPath $ProfileMap[$id]
        $exists = Test-Path -LiteralPath $abs -PathType Leaf
        # 徽标只描述三件不同的事，别混：现在选的是哪个、上次成功启动过哪个、哪个是推荐档。
        $badge = ''
        if ($id -eq $Session.ProfileId) { $badge = '当前选择' }
        elseif ($Session.Remembered -and $id -eq $Session.LastProfile) { $badge = '上次选择' }
        elseif ($id -eq $DefaultProfileId) { $badge = '推荐' }
        $hint = $ProfileHints[$id]
        if (-not $exists) {
            # 不静默退回另一个档位：把实际路径和原因说清楚。
            $hint = "缺失：$abs —— 请先恢复这个 profile，脚本不会替你换到另一个档位"
        }
        $items += @{
            Key = $id; Label = $ProfileLabels[$id]; Badge = $badge
            Hint = $hint; Disabled = (-not $exists)
        }
        if ($id -eq $Session.ProfileId) { $defaultIndex = $i }
        $i++
    }
    $items += @{ Key = 'back'; Label = '返回'; Badge = ''; Hint = $null; Disabled = $false }
    # 这一屏是从"启动服务"还是从"检查模型"进来的，说明白；否则用户会以为按错了。
    $subtitle = '当前档位已预选；选完立刻生效（下一次启动 / 检查用它）。Esc 返回。'
    if ($Reason) { $subtitle = "$Reason —— 当前档位已预选，Esc 取消。" }
    $sel = Show-ArrowMenu -Title '切换档位' -Subtitle $subtitle -Items $items -DefaultIndex $defaultIndex
    if (-not $sel -or $sel -eq 'back') { return $null }
    $Session.ProfileId = $sel
    return [pscustomobject]@{ Id = $sel; Path = (Resolve-RepoPath $ProfileMap[$sel]); Locked = $false }
}

function Show-AdvancedOptions {
    param([hashtable]$CliPlan, [hashtable]$Overrides, [hashtable]$Session)
    # 光标停在用户刚才那一行：动作做完菜单回来时光标不跳回第一行
    # （否则"下一次 Enter"会按到第一行 = 档位，用户看到的就是莫名跳出"切换档位"）。
    $start = 0
    while ($true) {
        $items = @()

        # 档位也能在这里直接切（用户问过"为什么没有切换挡位的功能"）：
        # 主菜单的两个动作会再问一次档位，这一行让"先切好档再启动"也能做到。
        $locked = $null -ne $CliPlan.ServeConfig
        $now = '(未定)'
        if ($Session.ProfileId -and $ProfileMap.Contains($Session.ProfileId)) { $now = $ProfileLabels[$Session.ProfileId] }
        elseif ($Session.LockedProfile) { $now = '由 -ServeConfig 指定' }
        $items += @{
            Key = 'profile'; Label = "档位            : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -ServeConfig 固定，菜单不覆盖它' } else { '回车选择：快速档 / 全功能档（下一次启动与检查都用它）' })
        }

        $locked = $null -ne $CliPlan.Port
        $now = if ($null -ne $Overrides.Port) { [string]$Overrides.Port }
               elseif ($locked) { [string]$CliPlan.Port }
               else { '(跟随 profile)' }
        $items += @{
            Key = 'port'; Label = "端口            : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -Port 固定，菜单不覆盖它' } else { '回车保持不变；范围 1-65535' })
        }

        $locked = $null -ne $CliPlan.Provider
        $now = if ($Overrides.ContainsKey('Provider')) { [string]$Overrides.Provider }
               elseif ($locked) { [string]$CliPlan.Provider }
               else { '(跟随 profile)' }
        $items += @{
            Key = 'provider'; Label = "provider        : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -Provider 固定，菜单不覆盖它' } else { '回车循环：跟随 profile → cpu → directml → cuda' })
        }

        $locked = $null -ne $CliPlan.TableEnabled
        $now = if ($Overrides.ContainsKey('TableEnabled')) { $(if ($Overrides.TableEnabled) { '启用' } else { '停用' }) }
               elseif ($locked) { $(if ($CliPlan.TableEnabled) { '启用' } else { '停用' }) }
               else { '(跟随 profile)' }
        $items += @{
            Key = 'table'; Label = "表格路由        : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -NoTable 固定，菜单不覆盖它' } else { '回车循环：跟随 profile → 启用 → 停用' })
        }

        $locked = $null -ne $CliPlan.Open
        $now = if ($Overrides.ContainsKey('Open')) { $(if ($Overrides.Open) { '打开' } else { '不打开' }) }
               elseif ($locked) { $(if ($CliPlan.Open) { '打开' } else { '不打开' }) }
               else { '(跟随 profile)' }
        $items += @{
            Key = 'open'; Label = "打开浏览器      : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -NoOpen 固定，菜单不覆盖它' } else { '回车循环：跟随 profile → 打开 → 不打开' })
        }

        $locked = $null -ne $CliPlan.NoBuild
        $now = if ($Overrides.ContainsKey('NoBuild')) { $(if ($Overrides.NoBuild) { '不构建（-NoBuild）' } else { '构建（--features serve）' }) }
               elseif ($locked) { $(if ($CliPlan.NoBuild) { '不构建（-NoBuild）' } else { '构建（--features serve）' }) }
               else { '构建（--features serve）' }
        $items += @{
            Key = 'build'; Label = "构建            : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -NoBuild 固定，菜单不覆盖它' } else { '回车切换：构建 ⇄ 不构建' })
        }

        $locked = ($null -ne $CliPlan.Download) -or ($null -ne $CliPlan.NoDownload)
        $now = if ($Overrides.ContainsKey('Download')) { '预先同意（-Download）' }
               elseif ($Overrides.ContainsKey('NoDownload')) { '从不下载（-NoDownload）' }
               elseif ($null -ne $CliPlan.Download) { '预先同意（-Download）' }
               elseif ($null -ne $CliPlan.NoDownload) { '从不下载（-NoDownload）' }
               else { '询问（缺模型时问一次）' }
        $items += @{
            Key = 'consent'; Label = "下载授权        : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -Download / -NoDownload 固定，菜单不覆盖它' } else { '回车循环：询问 → 预先同意 → 从不下载' })
        }

        $locked = $null -ne $CliPlan.DryRun
        $now = if ($Overrides.ContainsKey('DryRun')) { $(if ($Overrides.DryRun) { '是（只打印命令）' } else { '否' }) }
               elseif ($locked) { $(if ($CliPlan.DryRun) { '是（只打印命令）' } else { '否' }) }
               else { '否' }
        $items += @{
            Key = 'dryrun'; Label = "DryRun          : $now"; Disabled = $locked
            Hint = $(if ($locked) { '由命令行 -DryRun 固定，菜单不覆盖它' } else { '回车切换：否 ⇄ 是' })
        }

        $items += @{ Key = 'reset'; Label = '恢复默认（清空本次菜单覆盖）'; Badge = ''; Hint = '不影响命令行参数与 profile'; Disabled = $false }
        $items += @{ Key = 'back'; Label = '返回主菜单'; Badge = ''; Hint = $null; Disabled = $false }

        $sel = Show-ArrowMenu -Title '高级选项（只作用于本次启动）' -Subtitle '命令行已显式给出的项被锁定：菜单不会覆盖命令行。' -Items $items -DefaultIndex $start
        if (-not $sel -or $sel -eq 'back') { return }
        $start = $script:MenuLastIndex

        switch ($sel) {
            'profile' {
                # 选完只改会话里的当前档位；真正的生效点在启动 / 检查那一步（它负责把路径写进覆盖项）。
                $pick = Select-ServeProfile -Session $Session
                if ($pick) { Write-Host ("  档位已切换：{0}" -f $ProfileLabels[$Session.ProfileId]) -ForegroundColor Green }
            }
            'port' {
                # 端口也做成选择题：常见值直接选，只有"自定义"才需要手动输入。
                $profilePath = $defaultProfile
                if ($null -ne $CliPlan.ServeConfig) { $profilePath = Resolve-RepoPath $CliPlan.ServeConfig }
                elseif ($null -ne $Overrides.ServeConfig) { $profilePath = Resolve-RepoPath $Overrides.ServeConfig }
                $profilePort = $null
                try {
                    if (Test-Path -LiteralPath $profilePath -PathType Leaf) {
                        $profilePort = (Read-Utf8Json -Path $profilePath).port
                    }
                }
                catch { $profilePort = $null }

                $cur = if ($Overrides.ContainsKey('Port')) { [string]$Overrides.Port } else { '(跟随 profile)' }
                $items = @(
                    @{ Key = 'follow'; Label = '跟随 profile（推荐）'; Badge = $(if ($profilePort) { "profile: $profilePort" } else { '' }); Hint = '不覆盖 profile 里的端口'; Disabled = $false }
                )
                foreach ($p in @(8760, 8761, 8780, 8790)) {
                    $items += @{
                        Key = "$p"; Label = "端口 $p"
                        Badge = $(if ($p -eq $profilePort) { 'profile 里就是这个' } else { '' })
                        Hint = $null; Disabled = $false
                    }
                }
                $items += @{ Key = 'custom'; Label = '自定义端口…'; Badge = ''; Hint = '只有这一项需要手动输入'; Disabled = $false }

                $sel = Show-ArrowMenu -Title '端口' -Subtitle ("当前：{0}" -f $cur) -Items $items -DefaultIndex 0
                if ($sel -eq 'follow') {
                    $Overrides.Remove('Port')
                }
                elseif ($sel -eq 'custom') {
                    $def = if ($null -ne $Overrides.Port) { [string]$Overrides.Port }
                           elseif ($profilePort) { [string]$profilePort }
                           else { '8760' }
                    $v = Read-MenuValue -Prompt '端口' -Default $def
                    if ($v -match '^\d+$' -and [int]$v -ge 1 -and [int]$v -le 65535) {
                        $Overrides.Port = [int]$v
                    }
                    else {
                        Write-Host ''
                        Write-Host ("  端口不合法：{0}（要 1-65535 的整数）" -f $v) -ForegroundColor Red
                        Start-Sleep -Milliseconds 900
                    }
                }
                elseif ($sel) {
                    $Overrides.Port = [int]$sel
                }
            }
            'provider' {
                # ★ 括号不能省：参数模式下 `Get-MenuStateIndex -Current $cur + 1` 会把 `+ 1`
                # 当多余的位置参数丢掉，函数返回的永远是"当前值"的下标 → 每个切换项按回车
                # 都没反应（用户实测报的就是这个）。必须写成 (...)+1 再取模。
                $cur = if ($Overrides.ContainsKey('Provider')) { $Overrides.Provider } else { $null }
                $values = @($null, 'cpu', 'directml', 'cuda')
                $next = $values[((Get-MenuStateIndex -Values $values -Current $cur) + 1) % $values.Count]
                if ($null -eq $next) { $Overrides.Remove('Provider') } else { $Overrides.Provider = $next }
            }
            'table' {
                $cur = if ($Overrides.ContainsKey('TableEnabled')) { [bool]$Overrides.TableEnabled } else { $null }
                $values = @($null, $true, $false)
                $next = $values[((Get-MenuStateIndex -Values $values -Current $cur) + 1) % $values.Count]
                if ($null -eq $next) { $Overrides.Remove('TableEnabled') } else { $Overrides.TableEnabled = $next }
            }
            'open' {
                $cur = if ($Overrides.ContainsKey('Open')) { [bool]$Overrides.Open } else { $null }
                $values = @($null, $true, $false)
                $next = $values[((Get-MenuStateIndex -Values $values -Current $cur) + 1) % $values.Count]
                if ($null -eq $next) { $Overrides.Remove('Open') } else { $Overrides.Open = $next }
            }
            'build' {
                $cur = if ($Overrides.ContainsKey('NoBuild')) { [bool]$Overrides.NoBuild } else { $false }
                $values = @($false, $true)
                $Overrides.NoBuild = $values[((Get-MenuStateIndex -Values $values -Current $cur) + 1) % $values.Count]
            }
            'consent' {
                $cur = if ($Overrides.ContainsKey('Download')) { 'yes' }
                       elseif ($Overrides.ContainsKey('NoDownload')) { 'no' }
                       else { 'ask' }
                $values = @('ask', 'yes', 'no')
                $next = $values[((Get-MenuStateIndex -Values $values -Current $cur) + 1) % $values.Count]
                $Overrides.Remove('Download')
                $Overrides.Remove('NoDownload')
                if ($next -eq 'yes') { $Overrides.Download = $true }
                elseif ($next -eq 'no') { $Overrides.NoDownload = $true }
            }
            'dryrun' {
                $cur = if ($Overrides.ContainsKey('DryRun')) { [bool]$Overrides.DryRun } else { $false }
                $values = @($false, $true)
                $Overrides.DryRun = $values[((Get-MenuStateIndex -Values $values -Current $cur) + 1) % $values.Count]
            }
            'reset' { $Overrides.Clear() }
        }
    }
}

# 只读诊断：静态检查文件与端口，不起服务、不构建、不下载。
function Show-EnvironmentDiagnostics {
    param([hashtable]$CliPlan, [hashtable]$Overrides)
    $interactive = Test-InteractiveConsole
    Write-Host ''
    Write-Host '  环境与配置诊断（只读：不构建、不启动服务）' -ForegroundColor Cyan
    Write-Host ''
    Write-Host ("    PowerShell   : {0}（交互终端：{1}）" -f $PSVersionTable.PSVersion.ToString(), $(if ($interactive) { '是' } else { '否 —— 菜单是按键注入驱动的' })) -ForegroundColor DarkGray
    Write-Host ("    仓库根       : {0}" -f $root) -ForegroundColor DarkGray
    if (Test-Path -LiteralPath $exe -PathType Leaf) {
        $fi = Get-Item -LiteralPath $exe
        Write-Host ("    serve 二进制 : {0}（{1:N1} MB，{2:yyyy-MM-dd HH:mm}）" -f $exe, ($fi.Length / 1MB), $fi.LastWriteTime) -ForegroundColor DarkGray
    }
    else {
        Write-Host ("    serve 二进制 : {0}（不存在；启动时会按需构建）" -f $exe) -ForegroundColor DarkYellow
    }
    foreach ($id in $ProfileMap.Keys) {
        $abs = Resolve-RepoPath $ProfileMap[$id]
        $mark = if (Test-Path -LiteralPath $abs -PathType Leaf) { '存在' } else { '缺失' }
        Write-Host ("    profile {0,-4}: {1}（{2}）" -f $ProfileLabels[$id], $abs, $mark) -ForegroundColor DarkGray
    }
    $state = Get-ServeUiState
    Write-Host ("    UI 状态文件  : {0}" -f $state.Path) -ForegroundColor DarkGray
    Write-Host ("                   lastProfile={0}，来源={1}" -f $state.LastProfile, $state.Source) -ForegroundColor DarkGray
    if ($state.Warning) { Write-Host ("                   {0}" -f $state.Warning) -ForegroundColor DarkYellow }
    Write-Host ("    允许下载域名 : {0}" -f ($AllowedDownloadHosts -join ', ')) -ForegroundColor DarkGray
    Write-Host ("    HF_ENDPOINT  : {0}" -f $(if ($env:HF_ENDPOINT) { $env:HF_ENDPOINT } else { '(未设置)' })) -ForegroundColor DarkGray
    Write-Host ''

    $plan = Merge-ServePlan -CliPlan $CliPlan -Overrides $Overrides
    $resolved = $null
    try { $resolved = Resolve-ServePlan -Plan $plan }
    catch { Write-Host ("    生效配置     : ✗ {0}" -f $_.Exception.Message) -ForegroundColor Red }
    if ($resolved) {
        Write-Host ("    生效配置     : profile={0}" -f $resolved.ProfilePath) -ForegroundColor DarkGray
        Write-Host ("                   model-dir={0}" -f $resolved.ModelDirAbs) -ForegroundColor DarkGray
        Write-Host ("                   模型清单={0}" -f $(if ($resolved.ManifestFound) { $resolved.ManifestAbs } else { '(无 manifest.json，用内建默认模型表)' })) -ForegroundColor DarkGray
        Write-Host ("                   config={0}" -f $resolved.ConfigAbs) -ForegroundColor DarkGray
        Write-Host ("                   table={0}" -f $(if ($resolved.TableAbs) { $resolved.TableAbs } else { '(未启用)' })) -ForegroundColor DarkGray
        Write-Host ("                   provider={0}" -f $(if ($resolved.Provider) { $resolved.Provider } else { '(跟随 config)' })) -ForegroundColor DarkGray
        $owners = @(Get-PortOwner -LocalPort $resolved.Port)
        if ($owners.Count -gt 0) {
            $described = foreach ($owner in $owners) {
                $p = Get-Process -Id $owner -ErrorAction SilentlyContinue
                if ($p) { "$($p.ProcessName) (PID $owner)" } else { "PID $owner" }
            }
            Write-Host ("    端口 {0,-7}: 被占用：{1}" -f $resolved.Port, ($described -join ', ')) -ForegroundColor Yellow
            Write-Host ("                   要停掉它：Stop-Process -Id {0}（本脚本不会自动杀进程，也不会换端口）" -f ($owners -join ',')) -ForegroundColor DarkGray
        }
        else {
            Write-Host ("    端口 {0,-7}: 空闲" -f $resolved.Port) -ForegroundColor DarkGray
        }
    }
}

function Show-MainMenu {
    param([hashtable]$Session, [int]$DefaultIndex = 0)
    $label = '(未定)'
    $path = ''
    if ($Session.ProfileId -and $ProfileMap.Contains($Session.ProfileId)) {
        $label = $ProfileLabels[$Session.ProfileId]
        $path = Resolve-RepoPath $ProfileMap[$Session.ProfileId]
    }
    elseif ($Session.LockedProfile) {
        $label = '由 -ServeConfig 指定'
        $path = $Session.LockedProfile
    }
    # 徽标说清楚"现在用的是哪个档"：默认档写推荐，切过的写已切换（不然用户看不出切换生效了）。
    $badge = "推荐 · 档位：$label"
    if ($Session.LockedProfile) { $badge = "档位：$label" }
    elseif ($Session.ProfileId -ne $DefaultProfileId) { $badge = "档位：$label（已切换）" }
    $items = @(
        @{ Key = 'serve'; Label = '启动本地服务'; Badge = $badge; Hint = $path; Disabled = $false },
        @{ Key = 'models'; Label = '检查 / 下载模型'; Badge = ''; Hint = '盘点 OCR / 表格 / 公式三种模型，缺就给出下载地址（会临时起一次服务读 /api/models）'; Disabled = $false },
        @{ Key = 'diag'; Label = '环境与配置诊断'; Badge = ''; Hint = '路径 / profile / 端口占用 / 允许域名 / 状态文件；只读，不起服务'; Disabled = $false },
        @{ Key = 'advanced'; Label = '高级选项'; Badge = ''; Hint = '档位、端口、provider、表格路由、构建、下载授权、DryRun'; Disabled = $false },
        @{ Key = 'exit'; Label = '退出'; Badge = ''; Hint = $null; Disabled = $false }
    )
    $sel = Show-ArrowMenu -Title 'RapidStructure 服务管理器' -Subtitle '表格 / 公式识别跑在本地：模型缺失时先补齐，再启动。' -Items $items -DefaultIndex $DefaultIndex
    if (-not $sel) { return 'exit' }
    return $sel
}

function Invoke-ServeWizard {
    param([hashtable]$CliPlan)

    $state = Get-ServeUiState
    if ($state.Warning) {
        Write-Host ''
        Write-Host ("  {0}" -f $state.Warning) -ForegroundColor DarkYellow
    }

    $session = @{
        ProfileId     = $state.LastProfile
        # 状态文件里记下的档位（"上次成功启动过哪个"），与 ProfileId（"现在选的是哪个"）是两件事。
        LastProfile   = $state.LastProfile
        # 这个档位是**从状态文件里读出来的**（真的记过一次）还是仅仅"内建默认"？
        # 只有前者才配在菜单里写「上次选择」，否则误导成"你上次选过"。
        Remembered    = ($state.Source -eq 'file')
        LockedProfile = $(if ($CliPlan.ServeConfig) { Resolve-RepoPath $CliPlan.ServeConfig } else { $null })
        Overrides     = @{}
    }
    if ($session.LockedProfile) {
        $session.ProfileId = $null
        $session.Remembered = $false
        foreach ($id in $ProfileMap.Keys) {
            if ((Resolve-RepoPath $ProfileMap[$id]) -eq $session.LockedProfile) { $session.ProfileId = $id }
        }
    }

    # 菜单里最后一次操作的退出码（退出菜单时用它当本脚本的退出码）。
    $lastExit = 0
    $mainIndex = 0

    while ($true) {
        $choice = Show-MainMenu -Session $session -DefaultIndex $mainIndex
        $mainIndex = $script:MenuLastIndex
        if ($choice -eq 'exit') {
            # 退出码反映"本次菜单里最后一次操作"的结果：什么都没做就是 0，
            # 上次检查/启动失败就是那次失败的码（这样 `.\serve.ps1` 也能被脚本调用）。
            return [pscustomobject]@{ ExitCode = $lastExit; Started = $false }
        }

        if ($choice -eq 'diag') {
            Show-EnvironmentDiagnostics -CliPlan $CliPlan -Overrides $session.Overrides
            Wait-MenuContinue
            continue
        }
        if ($choice -eq 'advanced') {
            Show-AdvancedOptions -CliPlan $CliPlan -Overrides $session.Overrides -Session $session
            continue
        }

        # 启动服务 / 检查模型：先定档位（命令行给了 -ServeConfig 就不问），确认后再跑。
        # 检查模型也要说清"为什么问档位"——不问的话用户只会看到一屏"切换档位"，以为按错了。
        $pick = Select-ServeProfile -Session $session -Reason $(if ($choice -eq 'models') {
                '检查 / 下载模型：先确认查哪一档的模型目录（快速档 = small + 表格，全功能档 = medium + 公式 + 表格）'
            } else { '' })
        if (-not $pick) { continue }

        $overrides = @{}
        foreach ($k in $session.Overrides.Keys) { $overrides[$k] = $session.Overrides[$k] }
        if (-not $pick.Locked -and $pick.Id) { $overrides.ServeConfig = $pick.Path }
        $plan = Merge-ServePlan -CliPlan $CliPlan -Overrides $overrides

        if ($choice -eq 'models') {
            $mode = 'models-only'
            $modeTag = '只检查 / 补下模型，然后退出'
        }
        else {
            $mode = 'start'
            $modeTag = '启动本地网页服务（前台，Ctrl+C 停止）'
        }

        $resolved = $null
        try { $resolved = Resolve-ServePlan -Plan $plan }
        catch {
            Write-Host ''
            Write-Host ("  ✗ {0}" -f $_.Exception.Message) -ForegroundColor Red
            Wait-MenuContinue
            continue
        }

        # 确认之后换干净屏幕打印计划横幅：上一屏（菜单 + 确认项）不再残留在屏幕上。
        [void](Clear-MenuScreen)
        Show-ServePlanBanner -Resolved $resolved -ModeTag $modeTag
        if ($mode -eq 'models-only') {
            Write-Host '  说明：模型清单由服务自己的 GET /api/models 回答，所以这一步会临时起一次服务，' -ForegroundColor DarkYellow
            Write-Host '        读完就退出（进程清理与正式启动是同一段逻辑）。' -ForegroundColor DarkYellow
            Write-Host ''
        }
        if (-not (Wait-MenuConfirm)) {
            Write-Host '  已取消（没有启动任何东西）。' -ForegroundColor DarkGray
            Wait-MenuContinue
            continue
        }

        $result = $null
        try {
            # 档位标识交给第 4 层：它知道"服务就绪 + 模型齐备"这一刻在哪里，
            # 而那一刻才是"这次启动成功了"（比"前台退出之后"更早，Ctrl+C 也拦不住它）。
            $result = Invoke-ServeRun -Plan $plan -Mode $mode -ShowBanner:$false `
                    -ProfileId $(if ($mode -eq 'start' -and $pick.Id) { $pick.Id } else { $null })
        }
        catch {
            Write-Host ''
            Write-Host ("  ✗ {0}" -f $_.Exception.Message) -ForegroundColor Red
            Wait-MenuContinue
            continue
        }

        $lastExit = $result.ExitCode
        if ($result.Started) {
            # 启动成功 = 状态文件刚记下这个档位；会话里的"上次选择"同步跟上，
            # 返回主菜单时徽标与档位列表立刻是最新的。
            if ($mode -eq 'start' -and $pick.Id) {
                $session.LastProfile = $pick.Id
                $session.Remembered = $true
            }
            Write-Host ("  本次退出码：{0}" -f $result.ExitCode) -ForegroundColor DarkGray
        }
        else {
            Write-Host ("  本次没有起来（退出码 {0}）。" -f $result.ExitCode) -ForegroundColor DarkYellow
        }
        Wait-MenuContinue
    }
}

# ============================================================================
# 第 1 层：入口判定（菜单 or 命令行；非交互环境绝不等待按键）
# ============================================================================

$explicitKeys = @($PSBoundParameters.Keys | Where-Object { $_ -ne 'Menu' })
$interactive = Test-InteractiveConsole
Initialize-MenuInput
$injected = Test-MenuInjected

$menuWanted = $Menu -or ($explicitKeys.Count -eq 0)
if ($ModelsOnly) {
    # -ModelsOnly 是操作参数：与 -Menu 同给时以它为准，走命令行路径。
    $menuWanted = $false
}
$useMenu = $menuWanted -and ($interactive -or $injected)

if ($menuWanted -and -not $useMenu) {
    if ($Menu) {
        Write-Host 'serve: -Menu 需要可交互的终端；当前 stdin/stdout 被重定向或不可用。' -ForegroundColor Yellow
        Write-Host '      没有启动任何东西。命令行用法不变：.\serve.ps1 -ModelsOnly / -DryRun / -ServeConfig …' -ForegroundColor DarkGray
        exit 1
    }
    Write-Host 'serve: 无参数调用，但当前终端不可交互；按内建默认档位走命令行路径（不等待按键）。' -ForegroundColor DarkYellow
}

# 命令行显式值：$null = 没给。这样"没给"不会覆盖菜单的选择，而"给了"永远最优先。
$cliPlan = @{
    ServeConfig     = $(if ($PSBoundParameters.ContainsKey('ServeConfig')) { $ServeConfig } else { $null })
    Port            = $(if ($PSBoundParameters.ContainsKey('Port')) { $Port } else { $null })
    ModelDir        = $(if ($PSBoundParameters.ContainsKey('ModelDir')) { $ModelDir } else { $null })
    Config          = $(if ($PSBoundParameters.ContainsKey('Config')) { $Config } else { $null })
    TableModel      = $(if ($PSBoundParameters.ContainsKey('TableModel')) { $TableModel } else { $null })
    Provider        = $(if ($PSBoundParameters.ContainsKey('Provider')) { $Provider } else { $null })
    TableEnabled    = $(if ($PSBoundParameters.ContainsKey('NoTable')) { (-not $NoTable) } else { $null })
    Open            = $(if ($PSBoundParameters.ContainsKey('NoOpen')) { (-not $NoOpen) } else { $null })
    Reverify        = $(if ($PSBoundParameters.ContainsKey('Reverify')) { [bool]$Reverify } else { $null })
    NoBuild         = $(if ($PSBoundParameters.ContainsKey('NoBuild')) { [bool]$NoBuild } else { $null })
    DryRun          = $(if ($PSBoundParameters.ContainsKey('DryRun')) { [bool]$DryRun } else { $null })
    Download        = $(if ($PSBoundParameters.ContainsKey('Download')) { [bool]$Download } else { $null })
    NoDownload      = $(if ($PSBoundParameters.ContainsKey('NoDownload')) { [bool]$NoDownload } else { $null })
    PreferMirror    = $(if ($PSBoundParameters.ContainsKey('PreferMirror')) { [bool]$PreferMirror } else { $null })
    ReadyTimeoutSec = $ReadyTimeoutSec
    Rest            = $(if ($Rest) { $Rest } else { $null })
}

if ($useMenu) {
    $result = Invoke-ServeWizard -CliPlan $cliPlan
    if ($null -eq $result) { exit 0 }
    exit $result.ExitCode
}

try {
    $run = Invoke-ServeRun -Plan $cliPlan -Mode $(if ($ModelsOnly) { 'models-only' } else { 'start' })
}
catch {
    # 命令行路径也要一句人话，不要把 PowerShell 的堆栈原样摔给用户。
    Write-Host ''
    Write-Host ("  ✗ {0}" -f $_.Exception.Message) -ForegroundColor Red
    exit 1
}
exit $run.ExitCode