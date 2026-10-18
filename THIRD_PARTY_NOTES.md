# Third-party and model notes

`rapid-structure-rs` is Apache-2.0. The initial pipeline was derived from the local
`refer/paddle-ocr-rs-main` reference and keeps its Apache-2.0 notice. The
repository-level attribution is in `NOTICE`.

The following are project references or implementation influences, not a
claim that their model artifacts are licensed under this crate's license:

- `paddle-ocr-rs`: Apache-2.0 source reference.
- RapidOCR: Apache-2.0 source and deployment reference.
- PaddleOCR: upstream model and pipeline project; review its code and model
  terms separately for anything redistributed with an application.
- RapidDoc: Apache-2.0 source reference for PP-FormulaNet_plus preprocessing,
  tokenizer metadata handling, and LaTeX post-processing semantics.
- PP-FormulaNet / PaddleOCR / UniMER / im2latex: upstream model and dataset
  projects. Their weights, dictionaries, tokenizer metadata, and test images
  retain their own licenses and are not relicensed by this crate.

## Embedded web assets (the `serve` feature)

The optional `serve` feature embeds the following browser assets under
`src/bin/web/static/`. They are distributed as separate third-party works; the
crate's Apache-2.0 license does not relicense them.

- **KaTeX 0.19.0** (`katex.min.js`, `katex.min.css`): MIT License, from
  <https://github.com/KaTeX/KaTeX>.
- **KaTeX fonts** (`src/bin/web/static/katex/fonts/*.woff2`): MIT License,
  from <https://github.com/KaTeX/katex-fonts>.
- **MathJax 4.1.3** (`mathjax/tex-svg.js`): Apache License 2.0, from
  <https://github.com/mathjax/MathJax>. Its complete license text is
  `licenses/MathJax-Apache-2.0.txt`.

The complete MIT license texts for KaTeX and its fonts, and the complete
Apache-2.0 text for MathJax, are included under `licenses/`. A downstream
application can retain the applicable notices without
relying on a network connection. The source repositories and exact versions
are recorded here so the asset bundle can be reproduced.

## PP-FormulaNet_plus-M model

- ModelScope optimized artifact:
  `https://www.modelscope.cn/seeyuer/pp-formulanet-plus-m-optimized`
- Pinned file URL:
  `https://www.modelscope.cn/seeyuer/pp-formulanet-plus-m-optimized/resolve/0bb8154e3583d24613eaa74448de43ecf4ae73ec/pp_formulanet_plus_m.onnx`
- Artifact: graph-rewritten ONNX derived from RapidDoc `v1.0.0`; the model was
  not fine-tuned and learned weights are unchanged.
- SHA-256:
  `6e509ec4b7cda604dc9bf80a876ae6494693deea4c7f4157a4dc0eba0ebda745`
- On-disk size: 593,916,167 bytes (about 594 MB).
- ONNX contract: input `x` `FLOAT32 [N, 1, 384, 384]`, output `fetch_name_0`
  `INT64 [N, L]`, IR 10, opset 18, in-graph `Loop` whose maximum output width is
  2561 columns.
- Tokenizer: the model's `character` metadata embeds `fast_tokenizer_file`
  (vocab 50,000; `<s>`/`<pad>`/`</s>`/`<unk>` = 0/1/2/3) and
  `tokenizer_config_file`.
- Source model SHA-256:
  `71b6d389cf7b857e45252a4b98cfced1a3ffca7bf24d9497d02d052a41d9493b`
- License: the RapidDoc repository and upstream ModelScope model page identify
  Apache-2.0; the optimized repository includes its Apache-2.0 license text.
- The crate does not bundle the ~594 MB ONNX model. Applications must download
  it separately, verify the SHA-256, and accept the upstream terms.

## Pix2Text-MFD-1.5 page formula detector (optional)

Used only when `FormulaPolicy::detector_path` is set.

- File: `models/formula/detection/pix2text-mfd-1.5.onnx`
  (76.6 MB, not bundled).
- Source: Pix2Text / P2T (breezedeus) model card for `Pix2Text-MFD-1.5`.
- ONNX contract: input `images` `FLOAT32 [batch, 3, height, width]`, output
  `output0` `FLOAT32 [batch, 6, A]` (4 box channels + 2 class scores), IR 9,
  opset 19, metadata `imgsz=[768, 768]`, `stride=32`,
  `names={0: 'embedding', 1: 'isolated'}`.
- **License conflict that must be resolved by the consumer**: the model card
  front-matter declares `license: mit`, while the ONNX metadata retains
  `license: AGPL-3.0 License (https://ultralytics.com/license)` and
  `description: Ultralytics YOLO11m model`. These two statements are not
  compatible; do not redistribute this model until the upstream terms have been
  confirmed for the intended use.
- The crate does not bundle the detector. Detection accuracy on non-formula
  pages is limited; see the README's formula limitations section.

## SLANet-plus table structure model

Used by the table recognition path (`src/table/`, doc 07). Not bundled.

- File: `models/table/slanet-plus.onnx` (7,758,305 bytes, not bundled).
- Source URL:
  `https://www.modelscope.cn/models/RapidAI/RapidTable/resolve/v2.0.0/slanet-plus.onnx`
- Version: RapidAI/RapidTable `v2.0.0`.
- SHA-256:
  `d57a942af6a2f57d6a4a0372573c696a2379bf5857c45e2ac69993f3b334514b`
  (recorded in `models/table/manifest.json` next to the model, and verified on
  every load by `src/table/registration.rs` **before** any ONNX Runtime session
  is created).
- ONNX contract: input `x` `FLOAT32 [N, 3, H, W]` with `N`, `H` and `W` all
  dynamic; outputs `save_infer_model/scale_0.tmp_0` `FLOAT32 [N, L, 8]` (cell
  boxes) and `save_infer_model/scale_1.tmp_0` `FLOAT32 [N, L, 50]` (structure
  logits); IR 7, opset 14, `producer_name` and `producer_version` empty. `H` and
  `W` are pinned to 488 by the loader through ONNX Runtime's free-dimension
  override (`TableStructureConfig::fix_input_shape`); `L` is deliberately left
  dynamic. The graph contains a `Loop` whose trip cap constant is 500 and which
  exits early on the end-of-sequence token; it has 830 nodes.
- Vocabulary: the model's `character` metadata is a newline-separated list of 48
  structure fragments (not JSON), and the decoder adds `sos` / `eos` around it
  for 50 classes.
- License: the RapidTable **repository** ships an Apache-2.0 `LICENSE`
  (verified in the local reference checkout). The **weights** are
  `<待确认>`: upstream PaddleOCR publishes SLANet as training code and
  checkpoints, but there is no officially published SLANet ONNX artifact, so
  this file is a third-party re-packaging whose conversion script and exact
  source checkpoint are not documented, and the ONNX file itself carries no
  license metadata (`metadata_props` contains only `character`). Do not
  redistribute this model, and do not label it as this crate's Apache-2.0,
  until the upstream terms have been confirmed for the intended use.
- The crate does not bundle the ONNX model. Applications must download it
  separately, verify the SHA-256, and accept the upstream terms.



The following small fixtures are committed so that a clean clone can run the
contract and equivalence tests without any external asset:

- `tests/fixtures/formula-onnx/*.onnx` (KB-scale synthetic ONNX graphs, rebuilt
  by `tools/build_formula_onnx_fixtures.py`);
- `tests/fixtures/ocr-onnx/*.onnx`;
- `tests/fixtures/formula-golden/*` (PNG inputs + NumPy golden tensors);
- `tests/fixtures/formula-tokenizer/fast_tokenizer.json` (extracted from the
  PP-FormulaNet_plus-M `character` metadata) and `cases.json`;
- `tests/fixtures/formula-detect/*` (synthetic page images + golden boxes,
  rebuilt by `tools/formula_detect_reference.py`);
- `tests/fixtures/formula-postprocess/cases.json` (generated with the real
  `ftfy` by `tools/formula_ftfy_reference.py`).
- `tests/fixtures/table-onnx/*.onnx` (KB-scale synthetic ONNX graphs, rebuilt by
  `tools/build_table_onnx_fixtures.py`);
- `tests/fixtures/table-golden/*` — `table.jpg` (371×293), `input_bgr_resized.bin`
  (the 488×385 BGR buffer **after** `cv2.resize` and **before** normalize; baked in
  because `cv2.resize` is the one step that is not bit-reproducible across OpenCV
  builds and SIMD backends), `structure.npy` / `boxes.npy` (the reference model's two
  output tensors) and `manifest.json` (all metadata, per-stage coordinates, the
  reference token stream and the logical grid). Rebuilt by
  `tools/build_table_golden.py`, which loads the reference implementation in
  `refer/RapidTable-main` verbatim instead of re-implementing it.

`.gitignore` excludes all other `*.onnx` / `*.bin`, model caches, dataset copies,
and evaluation/benchmark outputs. The exceptions are exactly two:
`!tests/fixtures/**/*.onnx` and
`!tests/fixtures/table-golden/input_bgr_resized.bin` — the latter was added in the
table phase, after `cargo package --list` showed the file was silently missing from
the package: a `.bin` produced by a Python tool is not "a local model or a compiled
binary", so the blanket rule was wrong for it.

`tests/fixtures/table-golden/table.jpg` is the RapidTable repository's own test
image (`refer/RapidTable-main/tests/test_files/table.jpg`, byte-identical). That
repository ships an Apache-2.0 `LICENSE` (11,337 B, verified in the local checkout),
and the image is kept here as a test fixture with that origin recorded. The synthetic
`table-onnx` graphs and the NumPy goldens are generated by this repository's own
tools from that image and from the reference model's outputs; they carry no upstream
terms of their own. If the upstream repository's terms are ever found to exclude test
assets, replace this fixture with a generated image.

`unicode-normalization` (MIT/Apache-2.0) is the only dependency added for the
`ftfy` port; all `ftfy` tables are generated from the installed Python package
rather than transcribed by hand.

Runtime and image dependencies are consumed through Cargo. The important
runtime boundary is ONNX Runtime (`ort`/`ort-sys`), whose native DLL must be
distributed separately according to the selected feature and target.

Cargo dependencies retain their own licenses. A release bundle should produce
a dependency license inventory from `Cargo.lock` (for example with
`cargo-about`) rather than treating this crate's Apache-2.0 license as a
license for transitive dependencies.

The PP-OCRv6 detector, recognizer, and dictionary are model artifacts, not
source code. Their upstream URL, exact revision, SHA-256, and redistribution
terms must be recorded in the application model manifest before packaging.
The crate verifies SHA-256 for every artifact (weights **and** recognition
dictionaries) through the shared per-file validator `model_set::validate_model_files`,
which `ModelManifest::validate_files` wraps; it does not invent or embed
checksums for artifacts that are not present.

No cloud OCR service is used. Model downloads, when enabled by the application,
must use HTTPS and an explicit expected SHA-256.
