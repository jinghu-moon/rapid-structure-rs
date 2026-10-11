//! `GET /static/*` 的同源静态资源：公式渲染引擎（KaTeX / MathJax）。
//!
//! # 单一权威
//!
//! 资源表由 `build.rs` 扫描 `src/bin/web/static/` 生成（`$OUT_DIR/serve_assets.rs`），
//! 文件以 `include_bytes!` **编译期嵌入**，运行期不读盘、也没有第二份手工清单。
//! 构建期门禁同时校验 `katex.min.css` 的每一条 `url(fonts/…)` 引用都有对应的 woff2
//! 入库——缺文件是**构建失败**，不是运行期 404。
//!
//! # 路径安全来自封闭查表
//!
//! 请求路径**从不**参与文件系统操作：它只在生成表里做二分查找。`..`、反斜杠、
//! 百分号编码变体（本服务不做 URL 解码，见 `http::split_url`）都不可能命中任何
//! 表项，自然落到 404。因此这里不存在"静态目录穿越"这个问题类别。
//!
//! # 令牌边界
//!
//! `/static/*` 不在 `/api/*` 之下（§7.2：令牌对 `/api/*` 必需），资源本身无任何
//! 用户数据；页面 CSP 的 `font-src 'self'` 只为 KaTeX 字体放行同源。

/// build.rs 生成的 `(URL, MIME, bytes)` 表（按 URL 排序，可二分）。
mod generated {
    include!(concat!(env!("OUT_DIR"), "/serve_assets.rs"));
}

/// `URL → (MIME, 字节)`；不在表内返回 `None`（调用方按 404 处理）。
pub(super) fn lookup(path: &str) -> Option<(&'static str, &'static [u8])> {
    let index = generated::SERVE_ASSETS
        .binary_search_by(|(url, _, _)| (*url).cmp(path))
        .ok()?;
    let (_url, mime, bytes) = generated::SERVE_ASSETS[index];
    Some((mime, bytes))
}

/// 嵌入资源的数量（启动日志用：让"这次构建带没带渲染引擎"可见）。
pub(super) fn asset_count() -> usize {
    generated::SERVE_ASSETS.len()
}

#[cfg(test)]
mod tests {
    use super::{asset_count, generated, lookup};

    #[test]
    fn the_documented_engines_are_embedded_with_their_mime_types() {
        for (path, mime) in [
            (
                "/static/katex/katex.min.js",
                "application/javascript; charset=utf-8",
            ),
            ("/static/katex/katex.min.css", "text/css; charset=utf-8"),
            ("/static/katex/fonts/KaTeX_Main-Regular.woff2", "font/woff2"),
            (
                "/static/mathjax/tex-svg.js",
                "application/javascript; charset=utf-8",
            ),
        ] {
            let (embedded, bytes) =
                lookup(path).unwrap_or_else(|| panic!("{path} must be embedded"));
            assert_eq!(embedded, mime, "{path}");
            assert!(!bytes.is_empty(), "{path}");
        }
        let embedded = asset_count();
        assert!(embedded >= 23, "katex + fonts + mathjax: {embedded}");
    }

    /// 表是封闭的：任何目录穿越写法（原始或编码后）都不是表项。
    #[test]
    fn traversal_shapes_never_match_the_closed_table() {
        for path in [
            "/static/",
            "/static/../Cargo.toml",
            "/static/katex/../../Cargo.toml",
            "/static/%2e%2e/Cargo.toml",
            "/static\\katex\\katex.min.js",
            "/static/katex/katex.min.js/",
            "/static/katex/katex.min.jsx",
        ] {
            assert!(lookup(path).is_none(), "{path} must not resolve");
        }
    }

    /// 生成表的形状约束：URL 前缀、MIME 白名单、排序（二分的前提）。
    #[test]
    fn the_generated_table_is_sorted_and_within_the_allowed_shapes() {
        let urls: Vec<&str> = generated::SERVE_ASSETS
            .iter()
            .map(|(url, _, _)| *url)
            .collect();
        assert!(
            urls.windows(2).all(|pair| pair[0] < pair[1]),
            "table must be sorted and unique: {urls:?}"
        );
        for (url, mime, bytes) in generated::SERVE_ASSETS {
            assert!(url.starts_with("/static/"), "{url}");
            assert!(
                matches!(
                    *mime,
                    "application/javascript; charset=utf-8"
                        | "text/css; charset=utf-8"
                        | "font/woff2"
                ),
                "{url}: unexpected mime {mime}"
            );
            assert!(!bytes.is_empty(), "{url}");
        }
    }

    /// 页面与资源**必须同源演进**：`index.html` 的 `FX_LIBS` 引用的每个 `/static/…`
    /// 路径都必须在表里。改页面路径忘了放文件（或反之）在这里就失败，
    /// 而不是等到浏览器里点一下才看到 404。
    #[test]
    fn every_static_path_the_page_references_is_embedded() {
        let page = crate::serve::run::PAGE_TEMPLATE;
        let mut referenced = 0usize;
        for rest in page
            .match_indices("/static/")
            .map(|(index, _)| &page[index..])
        {
            let token: String = rest
                .chars()
                .take_while(
                    |c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '/' | '.' | '-' | '_'),
                )
                .collect();
            if token.ends_with(".js") || token.ends_with(".css") || token.ends_with(".woff2") {
                assert!(
                    lookup(&token).is_some(),
                    "the page references {token} but it is not embedded"
                );
                referenced += 1;
            }
        }
        assert!(
            referenced >= 3,
            "FX_LIBS js/css paths expected in the page, saw {referenced}"
        );
    }
}
