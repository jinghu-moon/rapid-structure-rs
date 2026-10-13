# Launch headless Chrome and assert the page's upload flow end to end (M5 regression check),
# including the Step 2 transparent text layer (layer built from `words=chars`, a real mouse drag
# across the spans, and every span checked against its own word polygon).
#
#   pwsh -NoProfile -File tools\run-page-flow-cdp.ps1 -Base http://127.0.0.1:8760
#   pwsh -NoProfile -File tools\run-page-flow-cdp.ps1 -Base http://127.0.0.1:8760 -Image 'datasets\text\03旋转与倾斜.png' -AssertTilt
#   pwsh -NoProfile -File tools\run-page-flow-cdp.ps1 -Base http://127.0.0.1:8760 -Image 'datasets\text\04表格与键值对.png' -Mode text-table -AssertTable
#
# -AssertTilt additionally requires at least one span to be measurably off-axis: pass it only for a
# tilted image, or the tilt claim would be asserted against a page that has nothing tilted.
# -Mode selects one of the page's own recognition modes (text / formula / text-table / formula-table).
# It goes through the page's mode row, so the server must have that role configured (formula detector,
# table structure model); when it has not, the page rightly refuses and this check reports the refusal.
# -AssertTable / -AssertFormula require the run to have actually produced that kind of region, so the
# table cell exit and the formula LaTeX exit are never claimed against a page carrying neither.
#
# It is the only check that catches the M5 bug class: the pre-fix page accepted the 202, never
# polled `GET /api/jobs/{id}`, and left `.stage.scanning` on forever with an empty region list.
param(
  [string]$Base = 'http://127.0.0.1:8760',
  [int]$CdpPort = 9347,
  [string]$Image = '',
  [string]$OutDir = '',
  [string]$Mode = '',
  [switch]$AssertTilt,
  [switch]$AssertTable,
  [switch]$AssertFormula
)
$ErrorActionPreference = 'Stop'
if (-not $OutDir) { $OutDir = Join-Path $PSScriptRoot '..\target\flow-gate\cdp-flow' }
if (-not $Image) { $Image = Join-Path (Split-Path -Parent $PSScriptRoot) 'datasets\text\01基础多位置文本.png' }
# Chrome resolves a relative --user path against its own working directory, never the shell's,
# so a relative -Image would silently produce "no file" and an empty run.
$Image = [System.IO.Path]::GetFullPath($Image)
# A UTF-8 file with no BOM is decoded as ANSI by Windows PowerShell 5.1, which turns the default
# image path (and any -Image argument it was handed) into mojibake. Chrome then reports "no such
# file" only as an empty run several minutes later, so refuse here, in words, instead.
if (-not (Test-Path -LiteralPath $Image)) {
  throw "image not found: $Image -- run this script with pwsh 7 (a UTF-8-capable shell); Windows PowerShell 5.1 decodes its Chinese default path as ANSI."
}
$chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
if (-not (Test-Path $chrome)) { throw "chrome not found: $chrome" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# A fresh profile per run: a leftover headless Chrome keeps the previous one locked.
$userData = Join-Path $OutDir ("chrome-profile-" + (Get-Date -Format 'HHmmss'))
New-Item -ItemType Directory -Force -Path $userData | Out-Null
$args = @(
  '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
  '--hide-scrollbars', '--disable-extensions',
  "--remote-debugging-port=$CdpPort", "--user-data-dir=$userData",
  'about:blank'
)
$proc = Start-Process -FilePath $chrome -ArgumentList $args -PassThru
$code = 1
try {
  Start-Sleep -Milliseconds 1800
  node (Join-Path $PSScriptRoot 'check-page-flow-cdp.mjs') --base $Base --cdp $CdpPort --image $Image --out $OutDir `
    @(if ($AssertTilt) { '--assert-tilt' }) `
    @(if ($AssertTable) { '--assert-table' }) `
    @(if ($AssertFormula) { '--assert-formula' }) `
    @(if ($Mode) { '--mode'; $Mode })
  $code = $LASTEXITCODE
} finally {
  if ($proc -and -not $proc.HasExited) { $proc.Kill() }
  Get-Process chrome -ErrorAction SilentlyContinue |
      Where-Object { $_.Path -eq $chrome } | ForEach-Object {
          try { if ($_.CommandLine -like "*$userData*") { $_.Kill() } } catch {}
      }
  Start-Sleep -Milliseconds 400
}
exit $code
