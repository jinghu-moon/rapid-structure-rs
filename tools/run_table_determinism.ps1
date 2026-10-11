# Determinism check: run the full val set N times with identical config and compare
# the *bytes* of the prediction file.  CPU ORT inference is expected to be
# deterministic, so a byte-identical prediction file is a stronger statement than
# "the three TEDS numbers happened to agree" -- it is what `docs/10-table-evaluation-protocol.md`
# §1 fixes epsilon = 0 on.
#
# Two drivers, same evidence:
#   * `-Manifest <path>`  -> `rapidstruct evaluate --manifest` (the doc 09 §9.2 acceptance path)
#   * no `-Manifest`      -> `table_eval --predictions/--report` (the E7 path)
param(
    [int]$Runs = 3,
    [string]$Manifest = '',
    [string]$Prefix = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if ($Prefix -eq '') {
    $Prefix = if ($Manifest -ne '') { 'e8-run' } else { 'e7-run' }
}

$exeName = if ($Manifest -ne '') { 'rapidstruct.exe' } else { 'table_eval.exe' }
$exe = Join-Path $root "target\release\$exeName"
if (-not (Test-Path $exe)) { throw "missing binary: $exe (run: cargo build --release --bins)" }
if ($Manifest -ne '' -and -not (Test-Path $Manifest)) { throw "missing manifest: $Manifest" }

# NOTE: this accumulator must NOT be called `$runs` -- PowerShell variable names are
# case-insensitive, so `$runs` *is* the `[int]$Runs` parameter and `@()` fails to assign
# ("cannot convert System.Object[] to System.Int32").
$results = @()
for ($i = 1; $i -le $Runs; $i++) {
    $pred = "Temp\$Prefix$i-pred.json"
    $rep = "Temp\$Prefix$i-report.json"
    Write-Host "=== run $i / $Runs ==="
    if ($Manifest -ne '') {
        # The manifest fixes the prediction path, so each run overwrites it; the report
        # records the digest of the file that run produced, which is the evidence.
        & $exe evaluate --manifest $Manifest --output $rep | Out-Null
    } else {
        & $exe --predictions $pred --report $rep
    }
    if ($LASTEXITCODE -ne 0) { throw "run $i failed with exit code $LASTEXITCODE" }

    $report = Get-Content $rep -Raw -Encoding UTF8 | ConvertFrom-Json
    $hash = $report.outputs.predictions.sha256
    $bytes = $report.outputs.predictions.bytes
    $results += [pscustomobject]@{
        run = $i
        pred = $report.outputs.predictions.path
        report = $rep
        bytes = $bytes
        sha256 = $hash
        selected = $report.dataset.selected
        decodable = $report.dataset.decodable
        failed = $report.dataset.failed
        epsilon = $report.protocol.repeat_epsilon
    }
    Write-Host "run $i predictions: $bytes bytes  sha256=$hash"
}

Write-Host ''
Write-Host '=== prediction digests ==='
$results | Format-Table -AutoSize | Out-String | Write-Host

$distinct = ($results | Select-Object -ExpandProperty sha256 | Sort-Object -Unique).Count
Write-Host "distinct prediction digests: $distinct / $Runs"
$epsilon = ($results | Select-Object -First 1).epsilon
if ($distinct -eq 1) {
    Write-Host "VERDICT: byte-identical across all $Runs runs -> the table structure path is deterministic (epsilon = $epsilon holds)."
} else {
    Write-Host 'VERDICT: predictions differ between runs -> determinism is NOT established.'
    Write-Host '         With epsilon = 0 that is a DEFECT, not noise; see docs/10-table-evaluation-protocol.md section 1.3.'
}

$results | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 "Temp\${Prefix}runs.json"
