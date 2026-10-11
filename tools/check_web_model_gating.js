/* 网页就绪判定的回归检查（E22 新增）。
 *
 * 为什么需要它：`src/bin/web/index.html` 的就绪判定是**前端独有**的逻辑，Rust 侧的
 * `cargo test --bin rapidstruct --features serve` 只能断言"页面字符串里有某个元素"，断言不了
 * "这份 /api/models 数据会不会把某个模式置灰"。E22 的缺陷（表格模式永久置灰）正是这类：
 * 服务端说 `routing: true`，页面却去 `sets[].files[]` 里按 role 找 `table_structure`，
 * 而 `--table-model` 指定的结构模型**按设计不在任何集合里** → 找不到就永久置灰。
 *
 * 做法：把 index.html 里的 `modelsReadyFor` 等函数**原样抽出来**（大括号配平，不复制逻辑），
 * 喂真实的 `/api/models` JSON；同时把修前的同名函数作为**反向对照**跑同一份数据 —— 对照必须
 * 失败，否则这个检查是空洞的。它不含依赖、只读源码，所以可以直接进仓库。
 *
 * 用法（先取一份 /api/models；token 与 Origin 的取法见 docs/05 §5.1）：
 *   node tools/check_web_model_gating.js --models Temp\models-live.json
 * 退出码：0 = 全部通过；1 = 有断言失败（含反向对照失效）。
 */
const fs = require("fs");

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}

const modelsPath = arg("models", null);
const pagePath = arg("page", "src/bin/web/index.html");
if (!modelsPath) {
  console.error(
    "用法：node tools/check_web_model_gating.js --models <api-models.json> [--page <index.html>]",
  );
  process.exit(2);
}
const html = fs.readFileSync(pagePath, "utf8");
const live = JSON.parse(fs.readFileSync(modelsPath, "utf8"));

/* --- 从 HTML 里按名字抽出函数 / 顶层 const（大括号配平） --- */
function extractFunction(src, name) {
  const start = src.indexOf(`function ${name}(`);
  if (start < 0) throw new Error(`function ${name} not found`);
  let i = src.indexOf("{", start);
  let depth = 0;
  for (; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}") {
      depth--;
      if (depth === 0) return src.slice(start, i + 1);
    }
  }
  throw new Error(`unbalanced braces in ${name}`);
}

function extractConst(src, name) {
  const start = src.indexOf(`const ${name} = `);
  if (start < 0) throw new Error(`const ${name} not found`);
  let depth = 0;
  for (let i = start; i < src.length; i++) {
    const c = src[i];
    if (c === "{" || c === "[" || c === "(") depth++;
    else if (c === "}" || c === "]" || c === ")") depth--;
    else if (c === ";" && depth === 0) return src.slice(start, i + 1);
  }
  throw new Error(`unbalanced in const ${name}`);
}

const names = [
  "modelsReadyFor",
  "modelFiles",
  "defaultRouteReason",
  "normalizeModels",
  "planRow",
  "demoInitialModels",
];
const pieces = [extractConst(html, "QUEUE_ROLES"), extractConst(html, "ROLE_ZH")];
for (const n of names) pieces.push(extractFunction(html, n));

/* 修前的同名函数（逐字来自 git 历史里的那一段，含它自己的角色表 —— 反向对照必须自足，
   不能借用今天已经删掉 `table` 条目的 `QUEUE_ROLES`，否则对照会"跟着修好"而失去意义）。
   它用来证明这个检查真的能失败。 */
const oldBody = `
function modelsReadyForOld(queue) {
  const OLD_QUEUE_ROLES = {
    text: ["detector", "recognizer", "dictionary"],
    formula: ["formula_recognizer"],
    table: ["table_structure"],
  };
  const files = modelFiles();
  if (files === null)
    return { ready: false, missing: [], fatal: true, reason: null };
  const need = OLD_QUEUE_ROLES[queue] || OLD_QUEUE_ROLES.text;
  const missing = [];
  need.forEach((role) => {
    const f = files.find((x) => x.role === role);
    const zh = ROLE_ZH[role] || role;
    if (!f) missing.push(zh);
    else if (f.state === "corrupt") missing.push(zh + "（损坏）");
    else if (f.state !== "present") missing.push(zh);
  });
  if (missing.length)
    return { ready: false, missing, fatal: false, reason: null };
  if (queue === "formula" || queue === "table") {
    const route = state.models && state.models[queue];
    if (route && route.routing === false) {
      return { ready: false, missing: [], fatal: false,
               reason: route.disabled_reason || defaultRouteReason(queue) };
    }
  }
  return { ready: true, missing: [], fatal: false, reason: null };
}
`;

const state = {};
const api = new Function(
  "state",
  pieces.join("\n") +
    oldBody +
    "\nreturn { modelsReadyFor, modelsReadyForOld, modelFiles, normalizeModels, demoInitialModels };",
)(state);

let failures = 0;
function check(label, got, want) {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (!ok) failures++;
  console.log(`${ok ? "[OK  ]" : "[FAIL]"} ${label}${ok ? "" : `\n         got  ${JSON.stringify(got)}\n         want ${JSON.stringify(want)}`}`);
}

/* ---------- A. 真实 /api/models（全功能档，表格路由已开、结构模型不在集合里） ---------- */
state.models = api.normalizeModels(live);
console.log(
  `live: sets=${live.sets.length} file_roles=[${live.sets
    .flatMap((s) => s.files.map((f) => f.role))
    .join(",")}] table.routing=${live.table.routing} complete=${live.table.complete}`,
);
check("A1 表格就绪（新判定）", api.modelsReadyFor("table"), {
  ready: true,
  missing: [],
  fatal: false,
  reason: null,
});
const before = api.modelsReadyForOld("table");
check("A2 表格置灰（旧判定，证明本检查能失败）", [before.ready, before.missing], [
  false,
  ["表格结构"],
]);
check("A3 文本仍就绪", api.modelsReadyFor("text").ready, true);
check("A4 公式仍就绪", api.modelsReadyFor("formula").ready, true);

/* ---------- B. 服务端明确关闭表格路由 ---------- */
const off = JSON.parse(JSON.stringify(live));
off.table.routing = false;
off.table.disabled_reason = "table routing is off: pass --table-model";
state.models = api.normalizeModels(off);
check("B1 routing=false → 不可用 + 显示服务端理由", api.modelsReadyFor("table"), {
  ready: false,
  missing: [],
  fatal: false,
  reason: "table routing is off: pass --table-model",
});
check(
  "B2 旧判定在 routing=false 时压根走不到路由分支（先被'集合里没有角色'拦下，理由是错的）",
  (() => {
    const r = api.modelsReadyForOld("table");
    return [r.ready, r.missing, r.reason];
  })(),
  [false, ["表格结构"], null],
);

/* ---------- C. 结构模型缺失 / 损坏 ---------- */
const miss = JSON.parse(JSON.stringify(live));
miss.table.complete = false;
miss.table.missing = ["slanet-plus.onnx"];
state.models = api.normalizeModels(miss);
check("C1 缺文件 → 不可用并点名文件", api.modelsReadyFor("table"), {
  ready: false,
  missing: ["slanet-plus.onnx"],
  fatal: false,
  reason: null,
});
const cor = JSON.parse(JSON.stringify(live));
cor.table.complete = false;
cor.table.corrupt = ["slanet-plus.onnx"];
state.models = api.normalizeModels(cor);
check("C2 损坏 → 文案区分损坏", api.modelsReadyFor("table").missing, ["slanet-plus.onnx（损坏）"]);

/* ---------- D. 演示数据（离线预览必须也能选表格） ---------- */
state.models = api.demoInitialModels();
check("D1 演示：文本就绪", api.modelsReadyFor("text").ready, true);
check("D2 演示：表格就绪（结构模型在集合之外）", api.modelsReadyFor("table").ready, true);
check("D3 演示：公式仍不可用（刻意的）", api.modelsReadyFor("formula").ready, false);
const owned = (state.models.sets || []).some((s) =>
  (s.files || []).some((f) => f.name === "slanet-plus.onnx"),
);
check("D4 演示：没有集合拥有表格模型（= 旧判定必然置灰的原因）", owned, false);
check("D5 演示：计划表能显示表格那一行", state.models.pipelines.table.plan[0].name, "slanet-plus.onnx");

console.log(failures ? `\nFAILURES = ${failures}` : "\nFAILURES = 0");
process.exit(failures ? 1 : 0);
