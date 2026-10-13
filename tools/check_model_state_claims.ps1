# 模型状态声明口径检查器（残余 P1 风险的机器化禁令）。
#
# 背景：`FileIdentity` 只看路径、体积、mtime、首尾各 64 KiB。同体积、同 mtime、
# 首尾窗口不变而**只改中段**的编辑，普通状态查询看不见。审核据此定下的验收口径是：
# 可以接受这条已声明的启发式限制，但**不得**把 `present` 或缓存命中写成
# “已核实当前磁盘内容”，也不得宣称“模型状态始终反映当前磁盘内容”或
# “完整性问题已彻底消除”。
#
# 这类禁令只靠人复查一轮就会漂移，所以把它做成机器判据。两条规则：
#
#   1. 禁止式：把文本按空行切成块、块内按 `。！？;；.` 切成**句子**（英文也按 `.` 切，
#      否则一整段英文会被当成一句，“上一句的 not”就能为下一句的宣称兜底），
#      在**同一句**里出现下列“越界短语”却没有否定字根（不 / 无 / 未 / 勿 / 拒 / 禁 / 非 /
#      越界 / not / never / cannot / without …）= 把性能启发式说成完整性结论 → FAIL（exit 1）。
#      匹配前先把 markdown 与空白噪声（`` ` `` * > # | ~ 引号 括号 连字符）压掉，
#      所以加粗包裹、跨行折行的短语与否定词都能对上；判据范围始终是**一句**，
#      不吃上一句的否定词 —— 长中文文档里“前向字符窗口”几乎必然撞到某个“不”字，
#      那样守卫等于没有（第一版就是这么漏的，SelfTest 里带了这一例）。
#   2. 必须式：口径声明本身必须在场。任何一个文件丢失了它的口径句 → FAIL。
#      只禁止越界措辞而不要求“必须说清”，挡不住“悄悄删掉免责声明”的回归。
#
# 短语数据自身的豁免：本脚本以及把短语当判据字面量的 Rust 测试文件，会在文件开头
# 声明 `claim-guard: phrases-are-data`；带该标记的文件跳过第 1 条（第 2 条照常）。
# 这不是放水——被豁免的文件里那些短语是**被禁止的说法**本身，不是宣称。
#
# `-SelfTest` 拿三组合成样本跑同一套判据：两处肯定式越界（其中一处刻意埋在“前面
# 几段全是否定词”的长文里）必须被判 FAIL，规范写法（否定式 + markdown 包裹 +
# 英文跨行折行）必须被判 PASS。任一不符 → exit 2。守卫失效比没有守卫更危险，
# 所以这条也顺带钉住“脚本按 UTF-8 读到了中文”：编码不对时中文短语匹配不到，
# SelfTest 立刻炸。
#
# 用法：
#   pwsh -NoProfile -File tools/check_model_state_claims.ps1
#   pwsh -NoProfile -File tools/check_model_state_claims.ps1 -SelfTest
#
# 退出码：0 = 全部 PASS；1 = 存在越界措辞或缺失口径句；2 = 用法错误 / SelfTest 不成立。

[CmdletBinding()]
param(
    [string]$Root,

    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

# 越界短语：肯定形式。写这些短语而不配否定语境，就是把启发式当成完整性结论。
# 注意：Windows PowerShell 5.1 不接受数组字面量里的尾随逗号，最后一项后面不能加 `,`。
$ForbiddenPhrases = @(
    '当前磁盘内容的确定性证明',
    '磁盘上的当前事实',
    '已核实当前磁盘内容',
    '已按磁盘当前内容核实',
    '始终反映当前磁盘',
    '永远报告当前磁盘',
    '状态永远报告',
    '内容完整性证明',
    '完整性保证',
    '实时保证',
    '每次都能发现被替换的模型',
    'integrity guarantee',
    'always reflects the current',
    'trusted at all times',
    'verified against the current content'
)

# 否定语境标记：同一句里命中任一，即认为该处在声明限制而不是宣称保证。
# 用**字根**而不是整词：中文否定写法太多（不把 / 不描述 / 不作 / 不写成…），
# 列全词表会误报；而一个字根在句内几乎不可能不是否定（“不确定地保证完整性”
# 这类句子本身就不该出现在仓库里）。SelfTest 里的反例会证明：上一句的否定词
# 污染不了本句。
$Negations = @(
    '不', '无', '未', '勿', '拒', '禁', '非', '越界', '避', '缺', 'null',
    'not', 'never', 'cannot', 'without', 'refus', 'no longer'
)

# 噪声字符：压掉之后再做匹配（保留 `。！？;；，：` 这些有语义的标点）。
$NoisePattern = '[\s*_>#|~`' + '"' + "'" + '“”‘’（）(){}\[\]「」『』-]'

$CleanPhrases = @($ForbiddenPhrases | ForEach-Object { $_ -replace $NoisePattern, '' })
$CleanNegations = @($Negations | ForEach-Object { $_ -replace $NoisePattern, '' })

# 必须存在的口径句（相对路径 -> 必须出现的子串数组）。
# 子串拿的是仓库里**实际在场**的稳定写法（不是理想措辞）：第一轮跑这条就抓出了
# 三处“我以为写了但其实没写”：model_verify/model_plan 里没有“性能启发式”而是
# “启发式”，model_plan 的口径句被文档注释折行拆成两行（所以只能拿“确定性证明”），
# docs/05 写的是“只有三个入口”而不是“三个确定性入口”。
$RequiredClaims = [ordered]@{
    'README.md' = @(
        'heuristic that narrows the window, not a security boundary',
        'Three deterministic entry',
        'real-time guarantee'
    )
    'src/model_verify.rs' = @('确定性证明', '启发式')
    'src/model_set.rs' = @('上一次完整校验')
    'src/bin/serve/model_plan.rs' = @('确定性证明', '启发式')
    'docs/05-local-web-demo-implementation.md' = @('只有三个入口', '已按磁盘当前内容核实')
    'docs/12-structure-refactor-plan.md' = @('内容完整性证明')
}

$ScanDirs = @('src', 'docs', 'tools', 'tests')
$ScanFiles = @('README.md', 'CHANGELOG.md')
$IncludeExtensions = @('.rs', '.md', '.ps1', '.mjs', '.js', '.html', '.json', '.py', '.yaml', '.yml')
$SkipDirNames = @('target', '.git', 'node_modules', 'Temp', '.spec-workflow', '.qoder')

function Resolve-Root {
    param([string]$Given)
    $base = if ([string]::IsNullOrWhiteSpace($Given)) { (Get-Location).ProviderPath } else { $Given }
    if (-not [System.IO.Path]::IsPathRooted($base)) {
        $base = Join-Path (Get-Location).ProviderPath $base
    }
    if (-not (Test-Path -LiteralPath $base -PathType Container)) {
        Write-Host "ERROR: root not found: $base" -ForegroundColor Red
        exit 2
    }
    return $base
}

function Get-ScanFiles {
    param([string]$Base)
    $files = @()
    foreach ($dir in $ScanDirs) {
        $full = Join-Path $Base $dir
        if (-not (Test-Path -LiteralPath $full -PathType Container)) { continue }
        foreach ($file in (Get-ChildItem -LiteralPath $full -File -Recurse)) {
            if ($IncludeExtensions -notcontains $file.Extension.ToLowerInvariant()) { continue }
            # 相对路径必须归一为正斜杠：$RequiredClaims 的键是仓库里的正斜杠写法，而
            # Windows 下 Substring 拿到的是反斜杠。不归一时 `Contains($rel)` 永远为假，
            # “必须句”判据会**静默失效**（只有根目录的 README/CHANGELOG 侥幸被检查）。
            # SelfTest 里两种分隔符都当作输入跑一遍，钉住这件事。
            $rel = ($file.FullName.Substring($Base.Length).TrimStart('\', '/')) -replace '\\', '/'
            $files += [pscustomobject]@{ Path = $file.FullName; Rel = $rel }
        }
    }
    foreach ($name in $ScanFiles) {
        $full = Join-Path $Base $name
        if (Test-Path -LiteralPath $full -PathType Leaf) {
            $files += [pscustomobject]@{ Path = $full; Rel = $name }
        }
    }
    return $files
}

function Test-IsExempt {
    param([string[]]$Lines)
    # 只看前 40 行：豁免必须是文件级的显式声明。
    $head = [Math]::Min(40, $Lines.Length)
    for ($i = 0; $i -lt $head; $i++) {
        if ($Lines[$i].Contains('claim-guard: phrases-are-data')) { return $true }
    }
    return $false
}

function Test-HasNegation {
    param([string]$Sentence)
    foreach ($negation in $CleanNegations) {
        if ($Sentence.IndexOf($negation, [System.StringComparison]::Ordinal) -ge 0) { return $true }
    }
    return $false
}

# 一个块（连续非空行）内：压噪声拼成一句流，按句末标点切开，逐句判“短语 vs 否定”。
function Get-BlockViolations {
    param([object[]]$Block, [string]$Rel)
    $violations = @()
    if ($Block.Count -eq 0) { return $violations }

    $starts = @()
    $joined = ''
    foreach ($entry in $Block) {
        $starts += $joined.Length
        $joined += $entry.Clean
    }

    $offset = 0
    foreach ($sentence in ($joined -split '[。！？;；.…]')) {
        $sentenceStart = $offset
        $offset += $sentence.Length + 1
        if ($sentence.Length -eq 0) { continue }
        $negated = Test-HasNegation $sentence
        for ($i = 0; $i -lt $CleanPhrases.Count; $i++) {
            $phrase = $CleanPhrases[$i]
            $at = $sentence.IndexOf($phrase, [System.StringComparison]::Ordinal)
            if ($at -lt 0) { continue }
            if ($negated) { continue }
            $abs = $sentenceStart + $at
            $lineIndex = 0
            for ($j = 0; $j -lt $Block.Count; $j++) {
                if ($starts[$j] -le $abs) { $lineIndex = $j } else { break }
            }
            $violations += [pscustomobject]@{
                File    = $Rel
                Line    = $Block[$lineIndex].Line
                Kind    = 'unsupported-claim'
                Phrase  = $ForbiddenPhrases[$i]
                Excerpt = $Block[$lineIndex].Raw
            }
        }
    }
    return $violations
}

function Get-FileViolations {
    param([string]$Path, [string]$Rel)
    $text = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)
    $lines = [regex]::Split($text, '\r?\n')
    $violations = @()

    if (-not (Test-IsExempt $lines)) {
        $block = @()
        for ($i = 0; $i -lt $lines.Length; $i++) {
            $line = $lines[$i]
            if ($line.Trim().Length -eq 0) {
                $violations += Get-BlockViolations -Block $block -Rel $Rel
                $block = @()
                continue
            }
            $block += [pscustomobject]@{
                Line  = $i + 1
                Raw   = $line.Trim()
                Clean = ($line -replace $NoisePattern, '')
            }
        }
        # 文件末尾没有空行，尾块必须单独 flush，否则最后一句永远不被检查。
        $violations += Get-BlockViolations -Block $block -Rel $Rel
    }

    # 内层括号是必需的：方法调用的参数列表里 `,` 会被当成**参数分隔符**，
    # `$x.Contains($a -replace '\\', '/')` 会变成“Contains 接收 2 个参数”而报错。
    $normalizedRel = $Rel -replace '\\', '/'
    if ($RequiredClaims.Contains($normalizedRel)) {
        foreach ($claim in $RequiredClaims[$normalizedRel]) {
            if ($text.IndexOf($claim, [System.StringComparison]::Ordinal) -lt 0) {
                $violations += [pscustomobject]@{
                    File    = $Rel
                    Line    = 0
                    Kind    = 'missing-statement'
                    Phrase  = $claim
                    Excerpt = 'the required scope statement is gone from this file'
                }
            }
        }
    }
    return $violations
}

function Get-ClaimViolations {
    param([object[]]$Files)
    $violations = @()
    foreach ($file in $Files) {
        $violations += Get-FileViolations -Path $file.Path -Rel ([string]$file.Rel)
    }
    return $violations
}

function Write-Violations {
    param([object[]]$Violations)
    foreach ($violation in $Violations) {
        $excerpt = [string]$violation.Excerpt
        if ($excerpt.Length -gt 150) { $excerpt = $excerpt.Substring(0, 150) + '…' }
        Write-Host ("FAIL: {0}:{1} [{2}] {3}" -f $violation.File, $violation.Line, $violation.Kind, $violation.Phrase) -ForegroundColor Red
        Write-Host ("      {0}" -f $excerpt) -ForegroundColor DarkGray
    }
}

# --- SelfTest：判据必须真的会咬人，也不许误伤规范的口径句 ---
if ($SelfTest) {
    $temp = Join-Path ([System.IO.Path]::GetTempPath()) ("model-state-claims-selftest-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temp -Force | Out-Null
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    try {
        $cases = @(
            [pscustomobject]@{
                Name = 'affirmative-claim'
                Rel = 'affirmative-claim.md'
                Expect = 'unsupported-claim'
                Text = "# claims`n`n模型状态始终反映当前磁盘内容，这是一份完整性保证。`n"
            },
            [pscustomobject]@{
                # 埋在“前面全是否定词”的长文里：窗口法在这里必然漏判，句子法不能。
                Name = 'affirmative-claim-after-negations'
                Rel = 'after-negations.md'
                Expect = 'unsupported-claim'
                Text = ("# claims`n`n它不是确定性证明，也不能当作完整性结论，禁止越界措辞。`n" +
                    "`n前文说了很多不是，可是下面这句是宣称：`n`n" +
                    "状态永远报告当前磁盘内容，缓存命中已核实当前磁盘内容。`n")
            },
            [pscustomobject]@{
                Name = 'declared-limitation'
                Rel = 'declared-limitation.md'
                Expect = 'clean'
                Text = ("# claims`n`n它**不是**当前磁盘内容的确定性证明，也**不是**一份完整性保证。`n" +
                    "The cache is a performance heuristic, not an`n" +
                    "integrity guarantee, so the service does **not** claim that the on-disk content is`n" +
                    "trusted at all times. It is not a real-time guarantee either.`n" +
                    "> 口径：不能把 present 写成已按磁盘当前内容核实；此类措辞都是越界的。`n")
            },
            [pscustomobject]@{
                # 英文句边界：上一句的 not 不能给下一句的宣称兜底。
                Name = 'affirmative-claim-across-english-sentences'
                Rel = 'english-sentences.md'
                Expect = 'unsupported-claim'
                Text = ("# claims`n`nThe cache is not a proof. It is an integrity guarantee.`n")
            },
            [pscustomobject]@{
                # 必须式也要真的会咬：口径句被删掉的文件（正斜杠路径）。
                Name = 'required-statement-missing-slash'
                Rel = 'src/model_set.rs'
                Expect = 'missing-statement'
                Text = "# stub`n`n这一份把口径句删掉了，只剩无关内容。`n"
            },
            [pscustomobject]@{
                # 同上，但路径用反斜杠：钉住分隔符归一化，否则必须式静默失效。
                Name = 'required-statement-missing-backslash'
                Rel = 'src\model_set.rs'
                Expect = 'missing-statement'
                Text = "# stub`n`n这一份把口径句删掉了，只剩无关内容。`n"
            }
        )

        Write-Host ''
        Write-Host 'Model state claim guard - SelfTest' -ForegroundColor Cyan
        $broken = @()
        foreach ($case in $cases) {
            $path = Join-Path $temp ($case.Name + '.md')
            [System.IO.File]::WriteAllText($path, $case.Text, $utf8NoBom)
            $files = @([pscustomobject]@{ Path = $path; Rel = $case.Rel })
            $all = @(Get-ClaimViolations -Files $files)
            if ($case.Expect -eq 'clean') {
                # 规范写法：禁止式一声不出（合成路径不在必须式名单里，不干扰这一例）。
                $hits = @($all | Where-Object { $_.Kind -eq 'unsupported-claim' })
                $ok = ($hits.Count -eq 0)
            } else {
                $hits = @($all | Where-Object { $_.Kind -eq $case.Expect })
                $ok = ($hits.Count -gt 0)
            }
            $expectation = $case.Expect
            Write-Host ("  {0,-38} expected {1,-18} got {2} hit(s)  {3}" -f `
                    $case.Name, $expectation, $hits.Count, $(if ($ok) { 'ok' } else { 'BROKEN' }))
            foreach ($hit in $hits) { Write-Host ("      caught: {0} | {1}" -f $hit.Phrase, $hit.Excerpt) -ForegroundColor DarkGray }
            if (-not $ok) { $broken += $case.Name }
        }
        if ($broken.Count -gt 0) {
            Write-Host ''
            Write-Host ("SELFTEST FAIL: the rule misbehaves on: {0}" -f ($broken -join ', ')) -ForegroundColor Red
            Write-Host 'A guard that silently stops biting is worse than no guard. Fix the rule (or the' -ForegroundColor Red
            Write-Host 'encoding of this file: the phrases must be read as UTF-8, hence the BOM).' -ForegroundColor Red
            exit 2
        }
        Write-Host 'SELFTEST PASS: unsupported claims and lost scope statements are both caught.' -ForegroundColor Green
    } finally {
        Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit 0
}

# --- 正式扫描 ---
$base = Resolve-Root $Root
$selfPath = if ($PSCommandPath) { (Resolve-Path -LiteralPath $PSCommandPath).ProviderPath } else { $null }
$files = @(Get-ScanFiles -Base $base) | Where-Object {
    if ($selfPath -and $_.Path -eq $selfPath) { return $false }
    $rel = $_.Rel -replace '\\', '/'
    foreach ($skip in $SkipDirNames) {
        if ($rel -match ('(^|/)' + [regex]::Escape($skip) + '/')) { return $false }
    }
    return $true
}

Write-Host ''
Write-Host 'Model state claim check' -ForegroundColor Cyan
Write-Host 'Rule: a sentence may use an out-of-scope phrase only inside its own negation; every listed file must still carry its scope statement.'
Write-Host ('Scanned {0} text file(s); {1} forbidden phrase(s), {2} required statement file(s).' -f `
        $files.Count, $ForbiddenPhrases.Count, $RequiredClaims.Count)
Write-Host ''

$violations = @(Get-ClaimViolations -Files $files)
if ($violations.Count -gt 0) {
    Write-Violations -Violations $violations
    Write-Host ''
    Write-Host 'Fix the wording, do not relax the rule: state what the identity-keyed cache actually' -ForegroundColor Yellow
    Write-Host 'proves (the digest of the last full verification of that path), and name the' -ForegroundColor Yellow
    Write-Host 'deterministic entry points that re-hash anyway. A file that legitimately carries' -ForegroundColor Yellow
    Write-Host 'these phrases as rule data declares `claim-guard: phrases-are-data` at its top.' -ForegroundColor Yellow
    exit 1
}

Write-Host 'PASS: no model-state claim outruns what the cache can prove.' -ForegroundColor Green
exit 0
