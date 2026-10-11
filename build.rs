//! 构建期信息：把 ort-sys 实际链接进来的 ONNX Runtime **静态库**记录下来。
//!
//! # 为什么需要它
//!
//! 本 crate 用默认 feature 构建时，`ort-sys` 采取的是 `rustc-link-lib=static=onnxruntime`
//! （见 `target/<profile>/build/ort-sys-*/output`）：**ONNX Runtime 被静态链接进可执行
//! 文件**，进程里根本没有 `onnxruntime.dll` 模块。此时
//! `GetModuleHandleW("onnxruntime.dll")` 必然失败，报告也就无法通过“运行库 DLL”来定位
//! 到底是哪一份 ORT。
//!
//! 因此这里把“链接进来的那份静态库”写进编译期环境变量：
//!
//! - [`LINK_LIB_BASE_ENV`]：ort 缓存目录（`.../ort.pyke.io/dfbin/<target>/`），
//!   每个分发版本一个内容哈希子目录；
//! - [`LINK_LIB_NAME_ENV`]：静态库文件名（`onnxruntime.lib`）；
//! - [`LINK_LIB_SIZE_ENV`]：该次链接真正使用的那份 `onnxruntime.lib` 的字节数。
//!
//! 运行期用“目录 + 文件名 + 体积”在缓存里唯一定位那份库文件并计算 SHA-256：两个已知
//! 分发哈希目录里的 `onnxruntime.lib` 体积不同（`341,152,186` 与 `363,923,500` 字节），
//! 因此体积本身就能把候选区分开，哈希则用来复核。找不到时报告给出可定位原因，
//! **不编造哈希**。
//!
//! # 信息从哪里来
//!
//! 两条来源，按可靠性排序：
//!
//! 1. `DEP_ORT_SYS_LINK`：cargo 为 `links = "..."` 的依赖准备的元数据变量。`ort-sys`
//!    目前没有声明 `links`，所以这条通常不存在；保留它是为了将来上游补上时自动生效。
//! 2. **扫描 `target/<profile>/build/` 下 `ort-sys-*` 的输出文件**（以及 `output`），
//!    找 `cargo:rustc-link-search=native=<dir>` 与随后出现的 `static=<name>`。
//!    这是构建期事实的直接证据，也正是 `cargo build -vv` 会打印的那两行。

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    emit_serve_assets();

    let Some(out_dir) = std::env::var_os("OUT_DIR").map(PathBuf::from) else {
        return;
    };
    let Some(raw) = capture_from_env().or_else(|| capture_from_build_dir(&out_dir)) else {
        // 没有链接信息（例如换成 load-dynamic、或依赖的构建脚本换了输出格式）：
        // 不设置任何变量，运行期会报告“没有链接信息”这一可定位原因。
        return;
    };

    let Some((base_dir, file_name)) = split_base_dir(&raw) else {
        return;
    };
    let Ok(metadata) = std::fs::metadata(&raw) else {
        return;
    };

    println!("cargo:rustc-env={LINK_LIB_BASE_ENV}={}", base_dir.display());
    println!("cargo:rustc-env={LINK_LIB_NAME_ENV}={file_name}");
    println!("cargo:rustc-env={LINK_LIB_SIZE_ENV}={}", metadata.len());
}

/// 编译期环境变量：ort 缓存里分发目录的公共父目录。
pub const LINK_LIB_BASE_ENV: &str = "RAPID_STRUCTURE_ORT_LINK_DIR";
/// 编译期环境变量：被链接的静态库文件名（通常是 `onnxruntime.lib`）。
pub const LINK_LIB_NAME_ENV: &str = "RAPID_STRUCTURE_ORT_LINK_NAME";
/// 编译期环境变量：被链接的静态库字节数。
pub const LINK_LIB_SIZE_ENV: &str = "RAPID_STRUCTURE_ORT_LINK_BYTES";

/// 优先使用 cargo 提供的依赖元数据变量（`ort-sys` 将来声明 `links` 时可用）。
fn capture_from_env() -> Option<PathBuf> {
    let raw = std::env::var_os("DEP_ORT_SYS_LINK")
        .or_else(|| std::env::var_os("ORT_SYS_LINK"))
        .or_else(|| std::env::var_os("ORT_LIB_LOCATION"))?;
    let candidate = PathBuf::from(raw);
    candidate.exists().then_some(candidate)
}

/// 扫描构建目录里 `ort-sys-*` 的输出，提取 `rustc-link-search` + `static=<name>`。
fn capture_from_build_dir(out_dir: &Path) -> Option<PathBuf> {
    // `OUT_DIR` = `<target>/<profile>/build/<our-crate>-<hash>/out`；
    // 依赖的构建输出都在它的上两级。
    let build_dir = out_dir.parent()?.parent()?;
    let entries = std::fs::read_dir(build_dir).ok()?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("ort-sys-"))
        })
        .collect();
    // 目录名带内容哈希，排序只是为了结果稳定。
    dirs.sort();

    let mut parts: Vec<(PathBuf, String)> = Vec::new();
    for dir in dirs {
        for file in ["output", "stderr", "stdout"] {
            let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
                continue;
            };
            if let Some(found) = parse_link_lines(&text) {
                parts.push(found);
            }
        }
    }
    // 同一份分发可能出现在多个构建目录里；只要任意一个文件真实存在就可信。
    parts.into_iter().find_map(|(dir, name)| {
        let candidate = dir.join(&name);
        candidate.exists().then_some(candidate)
    })
}

/// 从构建脚本输出里解析出被静态链接的库文件路径。
///
/// ort-sys 会打印成对的两行：
///
/// ```text
/// cargo:rustc-link-search=native=<dir>
/// cargo:rustc-link-lib=static=onnxruntime
/// ```
fn parse_link_lines(text: &str) -> Option<(PathBuf, String)> {
    let mut search_dir: Option<PathBuf> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(dir) = line.strip_prefix("cargo:rustc-link-search=native=") {
            search_dir = Some(PathBuf::from(dir));
            continue;
        }
        if let Some(name) = line.strip_prefix("cargo:rustc-link-lib=static=") {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            // MSVC 的静态库文件名等于 `name.lib`；已带扩展名时不重复追加。
            let file_name = if name.ends_with(".lib") {
                name.to_string()
            } else {
                format!("{name}.lib")
            };
            if let Some(dir) = search_dir.take() {
                return Some((dir, file_name));
            }
        }
    }
    None
}

/// 把 `<base>/<dist-hash>/<file>` 拆成 `(<base>, <file>)`。
///
/// `base` 只保留到 `dfbin/<target>` 这一层，运行期再去枚举它下面的分发目录。
fn split_base_dir(resolved: &Path) -> Option<(PathBuf, String)> {
    let file_name = resolved.file_name()?.to_string_lossy().to_string();
    let dist_dir = resolved.parent()?;
    let base_dir = dist_dir.parent()?.to_path_buf();
    Some((base_dir, file_name))
}

// ==================== serve 同源静态资源（公式渲染引擎） ====================

/// 静态资源目录（相对 crate 根）：`GET /static/<相对路径>` 的唯一来源。
///
/// 这里的文件全部是**编译期嵌入**（`include_bytes!`），运行期不读盘；路由表由
/// [`emit_serve_assets`] 扫描目录生成，不存在第二份手工清单（单一权威）。
const STATIC_DIR: &str = "src/bin/web/static";

/// 生成 `$OUT_DIR/serve_assets.rs`：按 URL 排序的 `(path, mime, bytes)` 静态表。
///
/// 只在启用 `serve` feature 时生成；任一约束不满足（未知扩展名、KaTeX 字体引用
/// 缺文件、目录为空）都**直接 panic 终止构建**，而不是运行期 404。
fn emit_serve_assets() {
    if std::env::var_os("CARGO_FEATURE_SERVE").is_none() {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let static_root = manifest_dir.join(STATIC_DIR);
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo"));

    let mut assets: Vec<(String, &'static str, PathBuf)> = Vec::new();
    collect_assets(&static_root, &static_root, &mut assets);
    assets.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        !assets.is_empty(),
        "serve 静态资源目录 {STATIC_DIR} 为空；公式渲染引擎（KaTeX/MathJax）依赖它"
    );
    assert_unique_urls(&assets);
    gate_katex_font_references(&static_root, &assets);

    let mut code = String::from(
        "// @generated by build.rs —— serve `GET /static/*` 的编译期资源表，勿手改。\n\
         pub(super) const SERVE_ASSETS: &[(&str, &str, &[u8])] = &[\n",
    );
    for (url, mime, file) in &assets {
        let escaped = file.display().to_string().replace('\\', "\\\\");
        code.push_str(&format!(
            "    ({url:?}, {mime:?}, include_bytes!(r\"{escaped}\")),\n"
        ));
        println!("cargo:rerun-if-changed={}", file.display());
    }
    code.push_str("];");
    std::fs::write(out_dir.join("serve_assets.rs"), code).expect("write serve_assets.rs");
}

fn collect_assets(root: &Path, dir: &Path, out: &mut Vec<(String, &'static str, PathBuf)>) {
    for entry in std::fs::read_dir(dir).expect("read static asset dir") {
        let path = entry.expect("read static asset entry").path();
        if path.is_dir() {
            collect_assets(root, &path, out);
            continue;
        }
        let mime = match path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
        {
            "js" => "application/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "woff2" => "font/woff2",
            other => panic!(
                "未知的静态资源扩展名 `{other}`：{}（只允许 js/css/woff2，新增类型需同步 \
                 serve::assets 的 MIME 表与测试）",
                path.display()
            ),
        };
        let relative = path.strip_prefix(root).expect("asset under static root");
        let url = format!(
            "/static/{}",
            relative
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        out.push((url, mime, path));
    }
}

/// URL 重复会让二分查找命中哪一份取决于排序，必须在构建期排除。
fn assert_unique_urls(assets: &[(String, &'static str, PathBuf)]) {
    for pair in assets.windows(2) {
        assert_ne!(
            pair[0].0, pair[1].0,
            "构建产物里的静态资源 URL 重复：{}",
            pair[0].0
        );
    }
}

/// 门禁：`katex.min.css` 的每条 `url(fonts/…)` 引用都必须有对应的 `.woff2` 入库。
///
/// KaTeX 的 `@font-face` 同时声明 woff2/woff/ttf 三种格式，浏览器只取第一个支持的；
/// 我们只嵌入 woff2，因此按**字体族**校验（每个被引用的族必须有 woff2）。
fn gate_katex_font_references(static_root: &Path, assets: &[(String, &'static str, PathBuf)]) {
    let css_path = static_root.join("katex").join("katex.min.css");
    let Ok(css) = std::fs::read_to_string(&css_path) else {
        return; // 未引入 KaTeX 时本门禁不适用。
    };
    let mut families: Vec<String> = Vec::new();
    for reference in css.split("url(").skip(1) {
        let Some(target) = reference.split([')', '"', '\'']).next() else {
            continue;
        };
        let Some(font) = target.strip_prefix("fonts/") else {
            continue;
        };
        let Some(family) = font.strip_suffix(".woff2").or_else(|| {
            font.strip_suffix(".woff")
                .or_else(|| font.strip_suffix(".ttf"))
        }) else {
            continue;
        };
        let name = format!("{family}.woff2");
        if !families.iter().any(|seen| seen == &name) {
            families.push(name);
        }
    }
    for family in families {
        let url = format!("/static/katex/fonts/{family}");
        assert!(
            assets.iter().any(|(asset_url, _, _)| asset_url == &url),
            "katex.min.css 引用了字体 `{family}`，但 {STATIC_DIR}/katex/fonts/ 里没有它的 \
             woff2；补文件或换版本后重新构建"
        );
    }
}
