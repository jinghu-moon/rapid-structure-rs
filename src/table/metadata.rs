//! SLANet 结构词表（`character` metadata）的解析。
//!
//! 这一层**不依赖 ONNX、不依赖任何外部资产**，因此干净 clone 就能验证它。
//!
//! # 实测事实（`models/table/slanet-plus.onnx`，7,758,305 B）
//!
//! `character` metadata 是**换行分隔**的 48 个 token（整串 582 个字符），
//! 不是公式域那种 JSON。这一点是**硬约束**而不是风格偏好：
//!
//! - 公式域 `character` 是 JSON（内嵌 `fast_tokenizer_file.model.vocab`），
//!   `src/formula/model_info.rs` 会 `serde_json::from_str` 它；
//! - 表格域若照抄这条解析，会把一个词表当成 JSON 去解析并**失败**；
//!   反过来，若把 JSON 当成换行分隔词表，会得到一个单元素的"词表"——
//!   一个不会报错、但会让后续解码**全错**的静默退化。
//!
//! 所以这里要求 metadata **至少含一个换行**，把"单行 JSON"直接拒掉。
//!
//! # 一个容易看反的事实：token 是**片段**，不是整标签
//!
//! 48 个 token 的实际内容是：
//!
//! ```text
//! 0..5   <thead>  </thead>  <tbody>  </tbody>  <tr>  </tr>
//! 6..8   <td      >         </td>
//! 9..27  " colspan=\"2\"" .. " colspan=\"20\""     (19 个，带前导空格)
//! 28..46 " rowspan=\"2\"" .. " rowspan=\"20\""     (19 个，带前导空格)
//! 47     <td></td>
//! ```
//!
//! 即：
//!
//! ```text
//! <td>              = <td  +  >                        (token 6 + token 7)
//! <td colspan="2">  = <td  +   colspan="2"  +  >       (token 6 + token 9 + token 7)
//! 空单元格           = <td></td>                        (token 47，单 token)
//! ```
//!
//! 词表里**同时**有 `<td`（token 6）、`>`（token 7）、`</td>`（token 8）与
//! `<td></td>`（token 47）——所以"词表不含 `<td>`"只对**完整的** `<td>` 成立。
//! 解码器必须**先拼接片段再解析 HTML**，不能按 token 做字符串相等匹配。
//! 这条修正了 `docs/07-table-recognition.md` §7.3 的措辞。
//!
//! `rowspan` / `colspan` 都从 **2** 起（1 是隐含默认，不需要 token），到 20 止。

use std::path::Path;

use crate::error::{RapidOcrError, Result};

/// 词表之外的两个特殊类别：起始符 `sos` 与结束符 `eos`。
///
/// 结构 logits 的末维 = `character` token 数 + 这个常量。
/// 实测：48 + 2 = 50，与真实模型 `scale_1.tmp_0` 的末维一致。
pub const STRUCTURE_SPECIAL_TOKEN_COUNT: usize = 2;

/// 解析 SLANet 的 `character` metadata。
///
/// 要求换行分隔且每行非空；**不做 `trim()`** —— token 的前导空格是内容的一部分
/// （`' colspan="2"'` 的第 0 个字符就是空格），只有 Windows 行尾的 `\r` 会被去掉。
pub fn parse_character_metadata(raw: &str) -> Result<Vec<String>> {
    if raw.is_empty() {
        return Err(RapidOcrError::Config(
            "table structure model `character` metadata is empty".to_string(),
        ));
    }
    if !raw.contains('\n') {
        return Err(RapidOcrError::Config(format!(
            "table structure model `character` metadata must be newline-separated, got a single \
             line of {} characters; the formula domain's JSON tokenizer metadata cannot be \
             reused here",
            raw.chars().count()
        )));
    }

    let mut tokens = Vec::new();
    for (index, line) in raw.split('\n').enumerate() {
        let token = line.strip_suffix('\r').unwrap_or(line);
        if token.is_empty() {
            return Err(RapidOcrError::Config(format!(
                "table structure model `character` metadata has an empty token at line {index}"
            )));
        }
        tokens.push(token.to_string());
    }
    Ok(tokens)
}

/// 校验词表宽度与结构 logits 末维一致。
///
/// 这是**跨来源的交叉校验**：词表来自 metadata，末维来自计算图。两者独立产生，
/// 因此能互相证伪。若不一致，解码时按类别下标取 token 必然越界或错位 ——
/// 与其在解码路径上悄悄算错，不如在契约层直接拒绝。
pub fn validate_vocabulary_width(
    tokens: &[String],
    structure_last_dim: usize,
    model_path: &Path,
) -> Result<()> {
    let expected = tokens.len() + STRUCTURE_SPECIAL_TOKEN_COUNT;
    if structure_last_dim != expected {
        return Err(RapidOcrError::Config(format!(
            "table structure model vocabulary width mismatch: `character` metadata has {} tokens \
             (+{STRUCTURE_SPECIAL_TOKEN_COUNT} special = {expected} classes) but the structure \
             output's last dim is {structure_last_dim} (model={})",
            tokens.len(),
            model_path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// 真实模型的 metadata 整串（582 字符），逐字节照抄自 `slanet-plus.onnx`。
    const REAL_METADATA: &str = concat!(
        "<thead>\n</thead>\n<tbody>\n</tbody>\n<tr>\n</tr>\n<td\n>\n</td>\n",
        " colspan=\"2\"\n colspan=\"3\"\n colspan=\"4\"\n colspan=\"5\"\n colspan=\"6\"\n",
        " colspan=\"7\"\n colspan=\"8\"\n colspan=\"9\"\n colspan=\"10\"\n colspan=\"11\"\n",
        " colspan=\"12\"\n colspan=\"13\"\n colspan=\"14\"\n colspan=\"15\"\n colspan=\"16\"\n",
        " colspan=\"17\"\n colspan=\"18\"\n colspan=\"19\"\n colspan=\"20\"\n",
        " rowspan=\"2\"\n rowspan=\"3\"\n rowspan=\"4\"\n rowspan=\"5\"\n rowspan=\"6\"\n",
        " rowspan=\"7\"\n rowspan=\"8\"\n rowspan=\"9\"\n rowspan=\"10\"\n rowspan=\"11\"\n",
        " rowspan=\"12\"\n rowspan=\"13\"\n rowspan=\"14\"\n rowspan=\"15\"\n rowspan=\"16\"\n",
        " rowspan=\"17\"\n rowspan=\"18\"\n rowspan=\"19\"\n rowspan=\"20\"\n<td></td>",
    );

    #[test]
    fn real_vocabulary_parses_into_forty_eight_fragments() {
        let tokens = parse_character_metadata(REAL_METADATA).expect("real metadata must parse");
        assert_eq!(tokens.len(), 48);
        assert_eq!(REAL_METADATA.chars().count(), 582);
        assert!(validate_vocabulary_width(&tokens, 50, Path::new("slanet-plus.onnx")).is_ok());
    }

    /// 结构 token 是**片段**：`<td` 与 `>` 是两个独立 token，而不是一个 `<td>`。
    #[test]
    fn tags_are_split_into_fragments() {
        let tokens = parse_character_metadata(REAL_METADATA).unwrap();
        assert_eq!(tokens[6], "<td");
        assert_eq!(tokens[7], ">");
        assert_eq!(tokens[8], "</td>");
        assert_eq!(tokens[47], "<td></td>");
        assert!(
            !tokens.iter().any(|t| t == "<td>"),
            "the vocabulary must not contain a whole `<td>` token"
        );
        // span token 的前导空格是内容的一部分，不能被 trim 掉。
        assert_eq!(tokens[9], " colspan=\"2\"");
        assert_eq!(tokens[28], " rowspan=\"2\"");
    }

    #[test]
    fn json_metadata_is_rejected() {
        let error =
            parse_character_metadata(r#"{"fast_tokenizer_file":{"model":{"type":"WordLevel"}}}"#)
                .expect_err("single-line JSON must be rejected");
        let message = error.to_string();
        assert!(message.contains("newline-separated"), "{message}");
        assert!(message.contains("JSON"), "{message}");
    }

    #[test]
    fn empty_and_blank_token_metadata_is_rejected() {
        assert!(parse_character_metadata("").is_err());
        assert!(parse_character_metadata("only-one-line").is_err());
        let error = parse_character_metadata("<td\n\n</td>").expect_err("blank token rejected");
        assert!(error.to_string().contains("empty token"), "{error}");
        // 尾随换行会产生一个空 token —— 同样是坏 metadata，不静默吞掉。
        assert!(parse_character_metadata("<td\n</td>\n").is_err());
    }

    #[test]
    fn windows_line_endings_are_tolerated() {
        let tokens = parse_character_metadata("<td\r\n>\r\n</td>").expect("crlf must parse");
        assert_eq!(tokens, vec!["<td", ">", "</td>"]);
    }

    #[test]
    fn vocabulary_width_mismatch_is_rejected() {
        let tokens = parse_character_metadata(REAL_METADATA).unwrap();
        let error = validate_vocabulary_width(&tokens, 51, Path::new("slanet-plus.onnx"))
            .expect_err("51 classes cannot match a 48-token vocabulary");
        let message = error.to_string();
        assert!(message.contains("48 tokens"), "{message}");
        assert!(message.contains("50 classes"), "{message}");
        assert!(message.contains("last dim is 51"), "{message}");
        assert!(message.contains("slanet-plus.onnx"), "{message}");
    }
}
