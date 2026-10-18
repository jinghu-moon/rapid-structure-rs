param(
    [string]$OutputDir = "target/modelscope/formula-pp-formulanet-plus-m-optimized",
    [string]$ModelPath = "models/formula/recognition/onnx/pp_formulanet_plus_m.optimized.onnx"
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$sourceModel = Join-Path $repoRoot $ModelPath
$outputPath = Join-Path $repoRoot $OutputDir
$expectedSize = 593916167
$expectedSha256 = "6e509ec4b7cda604dc9bf80a876ae6494693deea4c7f4157a4dc0eba0ebda745"
$cardTemplate = Join-Path $repoRoot "release/modelscope/formula-pp-formulanet-plus-m-optimized/README.md"
$licenseSource = Join-Path $repoRoot "references/formula-speed/RapidDoc/LICENSE"

if (-not (Test-Path -LiteralPath $sourceModel -PathType Leaf)) {
    throw "Optimized formula model not found: $sourceModel"
}

$item = Get-Item -LiteralPath $sourceModel
$sha256 = (Get-FileHash -LiteralPath $sourceModel -Algorithm SHA256).Hash.ToLowerInvariant()
if ($item.Length -ne $expectedSize -or $sha256 -ne $expectedSha256) {
    throw "Model fingerprint mismatch. Expected $expectedSize bytes / $expectedSha256; got $($item.Length) bytes / $sha256"
}

if (Test-Path -LiteralPath $outputPath) {
    throw "Output directory already exists; move it or choose another OutputDir: $outputPath"
}
if (-not (Test-Path -LiteralPath $cardTemplate -PathType Leaf)) {
    throw "Model card template not found: $cardTemplate"
}
if (-not (Test-Path -LiteralPath $licenseSource -PathType Leaf)) {
    throw "Upstream RapidDoc Apache-2.0 license not found: $licenseSource"
}

New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
Copy-Item -LiteralPath $sourceModel -Destination (Join-Path $outputPath "pp_formulanet_plus_m.onnx")
Copy-Item -LiteralPath $licenseSource -Destination (Join-Path $outputPath "LICENSE")
Copy-Item -LiteralPath $cardTemplate -Destination (Join-Path $outputPath "README.md")
Set-Content -LiteralPath (Join-Path $outputPath "SHA256SUMS") -Value "$expectedSha256  pp_formulanet_plus_m.onnx" -Encoding ascii

$stagedModel = Join-Path $outputPath "pp_formulanet_plus_m.onnx"
$stagedItem = Get-Item -LiteralPath $stagedModel
$stagedHash = (Get-FileHash -LiteralPath $stagedModel -Algorithm SHA256).Hash.ToLowerInvariant()
if ($stagedItem.Length -ne $expectedSize -or $stagedHash -ne $expectedSha256) {
    throw "Staged artifact fingerprint mismatch."
}

Write-Output "Prepared ModelScope upload directory: $outputPath"
Write-Output "Model: $($stagedItem.Length) bytes, SHA-256 $stagedHash"
