/* 表格 HTML 净化白名单生成器（docs/12 §4.2 P1-6 / N2 裁决：规范文件 + 生成脚本，生成物入库）。
 *
 * 唯一权威：assets/table_html_whitelist.json。本脚本读它，派生两份生成物并写回仓库：
 *   1. src/output/table_whitelist.rs        —— Rust 侧常量（src/output/html.rs 只消费它）
 *   2. src/bin/web/index.html 的标记区       —— 页面侧常量（generated:table-whitelist 标记之间）
 * 两份生成物都内嵌同一个 fingerprint（对权威规范规范化串的 sha256）。Rust 侧的 fingerprint
 * 测试从规范文件重算指纹，与两份生成物比对 —— 任一不一致即失败，而不是靠"记得同时改两边"。
 *
 * usage: node tools/generate-table-whitelist.mjs [--check]
 *   --check  : 只校验生成物是否与权威一致（不写文件）；有差异则非零退出（CI 用）。
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { createHash } from 'node:crypto';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const specPath = join(root, 'assets', 'table_html_whitelist.json');
const rsPath = join(root, 'src', 'output', 'table_whitelist.rs');
const htmlPath = join(root, 'src', 'bin', 'web', 'index.html');

const OPEN = '/* <generated:table-whitelist> */';
const CLOSE = '/* </generated:table-whitelist> */';

const spec = JSON.parse(readFileSync(specPath, 'utf8'));

// 规范化指纹输入：字段固定顺序、数组按权威文件书写的顺序 join。改动顺序会改变指纹（有意）。
function canonical(s) {
  return (
    'rapid-structure/table-html-whitelist/v1\n' +
    'tags=' + s.tags.join(',') + '\n' +
    'attrs=' + s.attrs.join(',') + '\n' +
    'min_span=' + s.min_span + '\n' +
    'max_span=' + s.max_span + '\n' +
    'text_policy=' + s.text_policy + '\n'
  );
}
const fingerprint = createHash('sha256').update(canonical(spec), 'utf8').digest('hex');

// —— Rust 生成物 ——
const rsTags = spec.tags.map((t) => `    "${t}",`).join('\n');
const rsAttrs = spec.attrs.map((t) => `"${t}"`).join(', ');
const rust = `//! 生成物，**请勿手改**（docs/12 §4.2 P1-6）。\n` +
  `//!\n` +
  `//! 唯一权威：` + 'assets/table_html_whitelist.json' + `；生成器：` + 'tools/generate-table-whitelist.mjs' + `。\n` +
  `//! 本文件与页面 ` + 'src/bin/web/index.html' + ` 的生成区共享同一 fingerprint，由\n` +
  `//! ` + 'src/output/html.rs' + ` 的 fingerprint 测试对拍三份。改规范后重新运行生成器。\n` +
  `#![allow(dead_code)] // 常量面向 Rust / 页面 / 测试多张面孔，任一单独构建里可能有未消费项。\n` +
  `\n` +
  `/// 规范化白名单的 sha256 指纹（对权威文件重算，见生成器）。\n` +
  `#[rustfmt::skip] // 生成物：整行按生成器写法保留，不让 cargo fmt 回绕，保证 --check 字节对拍。\n` +
  `pub(crate) const FINGERPRINT: &str = "${fingerprint}";\n` +
  `\n` +
  `/// 净化白名单标签集合，` + '`[x 顺序即权威顺序]`' + `。\n` +
  `#[rustfmt::skip]\n` +
  `pub(crate) const ALLOWED_TABLE_TAGS: &[&str] = &[\n${rsTags}\n];\n` +
  `\n` +
  `/// 净化白名单属性（只有整数 span 属性；align/valign 已按安全交集移除）。\n` +
  `#[rustfmt::skip]\n` +
  `pub(crate) const ALLOWED_TABLE_ATTRS: &[&str] = &[${rsAttrs}];\n` +
  `\n` +
  `/// span 属性下界（含）。\n` +
  `pub(crate) const MIN_SPAN: u32 = ${spec.min_span};\n` +
  `\n` +
  `/// span 属性上界（含）。相对旧库实现由 1000 收紧到 ${spec.max_span}。\n` +
  `pub(crate) const MAX_SPAN: u32 = ${spec.max_span};\n` +
  `\n` +
  `/// 单元格文本策略（元数据；净化前置条件由引擎的 Escape 口径保证）。\n` +
  `pub(crate) const TEXT_POLICY: &str = "${spec.text_policy}";\n`;

// —— 页面生成区 ——
const jsTags = spec.tags.map((t) => `"${t}"`).join(', ');
const jsAttrs = spec.attrs.map((t) => `"${t}"`).join(', ');
const jsBlock =
  `${OPEN}\n` +
  `        const TABLE_ALLOWED_TAGS = [${jsTags}];\n` +
  `        const TABLE_ALLOWED_ATTRS = [${jsAttrs}];\n` +
  `        const TABLE_MIN_SPAN = ${spec.min_span};\n` +
  `        const TABLE_MAX_SPAN = ${spec.max_span};\n` +
  `        /* fingerprint=rapid-structure/table-html-whitelist/v1 sha256=${fingerprint} */\n` +
  `        ${CLOSE}`;

const check = process.argv.includes('--check');

// Rust 文件：整体生成。
if (check) {
  const current = readFileSync(rsPath, 'utf8');
  if (current !== rust) {
    console.error('table_whitelist.rs is out of sync with the spec. Re-run: node tools/generate-table-whitelist.mjs');
    process.exit(1);
  }
} else {
  writeFileSync(rsPath, rust, 'utf8');
}

// 页面：只替换标记区。
let html = readFileSync(htmlPath, 'utf8');
const openIdx = html.indexOf(OPEN);
const closeIdx = html.indexOf(CLOSE);
if (openIdx < 0 || closeIdx < 0 || closeIdx < openIdx) {
  console.error('generated:table-whitelist markers not found in index.html');
  process.exit(1);
}
const updated = html.slice(0, openIdx) + jsBlock + html.slice(closeIdx + CLOSE.length);
if (check) {
  if (html !== updated) {
    console.error('index.html table-whitelist region is out of sync with the spec. Re-run: node tools/generate-table-whitelist.mjs');
    process.exit(1);
  }
} else {
  writeFileSync(htmlPath, updated, 'utf8');
  html = updated;
}

if (!check) {
  console.log(`generated table whitelist; fingerprint=${fingerprint}`);
  console.log(`  tags=${spec.tags.length} attrs=${spec.attrs.length} span=${spec.min_span}..=${spec.max_span} policy=${spec.text_policy}`);
} else {
  console.log(`table whitelist in sync; fingerprint=${fingerprint}`);
}
