use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

#[cfg(test)]
use ndarray::Array3;
use ndarray::{ArrayView2, ArrayView3, Axis, s};

use crate::{
    error::{RapidOcrError, Result},
    ocr::{
        rec::align::char_column_ranges,
        types::{AlignStatus, WordInfo, WordType},
    },
};

#[derive(Debug, Clone)]
pub struct CtcLabelDecoder {
    character: Vec<String>,
}

pub type DecodedLine = (String, f32);
pub type DecodeOutput = (Vec<DecodedLine>, Vec<WordInfo>);

impl CtcLabelDecoder {
    pub fn new(character: Option<Vec<String>>, character_path: Option<&Path>) -> Result<Self> {
        let mut character_list = if let Some(character) = character {
            character
        } else if let Some(path) = character_path {
            read_character_file(path)?
        } else {
            return Err(RapidOcrError::Decode(
                "character and character_path are both None".to_string(),
            ));
        };

        if character_list.is_empty() {
            return Err(RapidOcrError::Decode("character list is empty".to_string()));
        }

        character_list.insert(0, "blank".to_string());
        character_list.push(" ".to_string());
        Ok(Self {
            character: character_list,
        })
    }

    #[cfg(test)]
    pub fn decode(
        &self,
        preds: &Array3<f32>,
        return_word_box: bool,
        wh_ratio_list: &[f32],
        max_wh_ratio: f32,
    ) -> Result<DecodeOutput> {
        self.decode_view(preds.view(), return_word_box, wh_ratio_list, max_wh_ratio)
    }

    pub fn decode_view(
        &self,
        preds: ArrayView3<'_, f32>,
        return_word_box: bool,
        wh_ratio_list: &[f32],
        max_wh_ratio: f32,
    ) -> Result<DecodeOutput> {
        if preds.ndim() != 3 {
            return Err(RapidOcrError::Decode(format!(
                "preds must be rank 3, got rank {}",
                preds.ndim()
            )));
        }

        let (batch_size, _timesteps, class_num) = preds.dim();
        if class_num == 0 {
            return Err(RapidOcrError::Decode(
                "preds class dim cannot be zero".to_string(),
            ));
        }
        if return_word_box && wh_ratio_list.len() != batch_size {
            return Err(RapidOcrError::InvalidInput(format!(
                "wh_ratio_list length {} does not match batch size {} when return_word_box=true",
                wh_ratio_list.len(),
                batch_size
            )));
        }

        let mut line_results = Vec::with_capacity(batch_size);
        let mut word_results = Vec::with_capacity(batch_size);

        for batch_idx in 0..batch_size {
            let probs = preds.slice(s![batch_idx, .., ..]);
            let (token_indices, token_probs) = argmax_with_prob(probs);

            let mut selection = vec![true; token_indices.len()];
            if token_indices.len() >= 2 {
                for i in 1..token_indices.len() {
                    selection[i] = token_indices[i] != token_indices[i - 1];
                }
            }

            for (idx, token) in token_indices.iter().enumerate() {
                if *token == 0 {
                    selection[idx] = false;
                }
            }

            let mut conf_list: Vec<f32> = token_probs
                .iter()
                .zip(selection.iter())
                .filter_map(|(v, keep)| if *keep { Some(round5(*v)) } else { None })
                .collect();

            if conf_list.is_empty() {
                conf_list.push(0.0);
            }

            let mut text = String::new();
            // 选中的 token id（按文本顺序）。它就是对齐的目标标签序列：blank 已被
            // `selection` 排除，所以序列里不会出现 0 号位。
            let mut selected_ids: Vec<usize> = Vec::with_capacity(token_indices.len());
            for (token, keep) in token_indices.iter().zip(selection.iter()) {
                if !*keep {
                    continue;
                }
                let ch = self.character.get(*token).ok_or_else(|| {
                    RapidOcrError::Decode(format!(
                        "token id {} out of character range {}",
                        token,
                        self.character.len()
                    ))
                })?;
                text.push_str(ch);
                selected_ids.push(*token);
            }

            // Keep parity near text_score threshold by aggregating in f64 before round-to-5.
            let sum_conf: f64 = conf_list.iter().map(|v| f64::from(*v)).sum();
            let mean_conf = round5_f64(sum_conf / conf_list.len() as f64);

            if return_word_box {
                // 逐字符 forced alignment：直接用**完整的 T×C 概率面**（不是 argmax 后的
                // 序列）在 2L+1 状态图上取 Viterbi 路径，于是重复字符能各自拿到区间。
                // 目标序列就是上面选中的 token；若字符表里存在多字符条目，文本长度与
                // token 数就会不一致，此时下游按字符编号的映射全部失效，直接记账退回。
                let alignable = text.chars().count() == selected_ids.len();
                let ranges = if alignable {
                    char_column_ranges(probs, &selected_ids)
                } else {
                    None
                };
                let mut info = get_word_info(&text, &selection, ranges.as_deref());
                let wh_ratio = *wh_ratio_list.get(batch_idx).unwrap_or(&1.0_f32);
                info.line_txt_len = token_indices.len() as f32 * wh_ratio / max_wh_ratio;
                info.confs = conf_list;
                word_results.push(info);
            }
            line_results.push((text, mean_conf));
        }

        Ok((line_results, word_results))
    }
}

fn argmax_with_prob(probs: ArrayView2<'_, f32>) -> (Vec<usize>, Vec<f32>) {
    let rows = probs.len_of(Axis(0));
    let cols = probs.len_of(Axis(1));
    let mut idxs = Vec::with_capacity(rows);
    let mut vals = Vec::with_capacity(rows);

    if let Some(slice) = probs.as_slice_memory_order() {
        for r in 0..rows {
            let row_start = r * cols;
            let row = &slice[row_start..row_start + cols];
            let mut max_idx = 0usize;
            let mut max_val = f32::NEG_INFINITY;
            for (idx, value) in row.iter().enumerate() {
                if *value > max_val {
                    max_val = *value;
                    max_idx = idx;
                }
            }
            idxs.push(max_idx);
            vals.push(max_val);
        }
        return (idxs, vals);
    }

    for row in probs.axis_iter(Axis(0)) {
        let mut max_idx = 0_usize;
        let mut max_val = f32::NEG_INFINITY;
        for (idx, value) in row.iter().enumerate() {
            if *value > max_val {
                max_val = *value;
                max_idx = idx;
            }
        }
        idxs.push(max_idx);
        vals.push(max_val);
    }
    (idxs, vals)
}

fn read_character_file(path: &Path) -> Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    Ok(out)
}

/// 把文本与选中列切成词，并（在对齐可用时）把逐字符列区间按同样的分组带出来。
///
/// `ranges` 的下标是**文本字符序号**（含空白），与 `valid_col` 同轴；空白字符本身
/// 不进入 `words`，所以也不进入 `char_cols`。返回的 `char_cols` 与 `words`
/// 逐层等长，下游可以直接 zip，不需要重建下标映射。
fn get_word_info(text: &str, selection: &[bool], ranges: Option<&[[usize; 2]]>) -> WordInfo {
    let mut word_list: Vec<Vec<String>> = Vec::new();
    let mut word_col_list: Vec<Vec<usize>> = Vec::new();
    let mut word_range_list: Vec<Vec<[usize; 2]>> = Vec::new();
    let mut state_list: Vec<WordType> = Vec::new();

    let mut word_content: Vec<String> = Vec::new();
    let mut word_col_content: Vec<usize> = Vec::new();
    let mut word_range_content: Vec<[usize; 2]> = Vec::new();

    let valid_col: Vec<usize> = selection
        .iter()
        .enumerate()
        .filter_map(|(idx, keep)| if *keep { Some(idx) } else { None })
        .collect();

    if valid_col.is_empty() {
        return WordInfo::default();
    }

    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return WordInfo::default();
    }

    // 只有长度对得上才接受这张区间表；对不上就当对齐没发生，避免把区间贴到错误的字上。
    let ranges: Option<&[[usize; 2]]> = ranges.filter(|r| r.len() == chars.len());
    let mut range_shape_ok = ranges.is_some();

    let mut col_width = vec![0_usize; valid_col.len()];
    for i in 1..valid_col.len() {
        col_width[i] = valid_col[i] - valid_col[i - 1];
    }
    let first_width = if has_chinese_char(chars[0]) { 3 } else { 2 };
    col_width[0] = first_width.min(valid_col[0]);

    let mut state: Option<WordType> = None;
    for (c_i, ch) in chars.iter().enumerate() {
        if ch.is_whitespace() {
            if !word_content.is_empty() {
                word_list.push(word_content.clone());
                word_col_list.push(word_col_content.clone());
                word_range_list.push(word_range_content.clone());
                state_list.push(state.unwrap_or(WordType::EnNum));
                word_content.clear();
                word_col_content.clear();
                word_range_content.clear();
            }
            continue;
        }

        let c_state = if has_chinese_char(*ch) {
            WordType::Cn
        } else {
            WordType::EnNum
        };

        if state.is_none() {
            state = Some(c_state);
        }

        if state != Some(c_state) || col_width.get(c_i).copied().unwrap_or(0) > 5 {
            if !word_content.is_empty() {
                word_list.push(word_content.clone());
                word_col_list.push(word_col_content.clone());
                word_range_list.push(word_range_content.clone());
                state_list.push(state.unwrap_or(WordType::EnNum));
                word_content.clear();
                word_col_content.clear();
                word_range_content.clear();
            }
            state = Some(c_state);
        }

        word_content.push(ch.to_string());
        if let Some(col) = valid_col.get(c_i) {
            word_col_content.push(*col);
        }
        if let Some(range) = ranges.and_then(|r| r.get(c_i)) {
            word_range_content.push(*range);
        } else {
            range_shape_ok = false;
        }
    }

    if !word_content.is_empty() {
        word_list.push(word_content);
        word_col_list.push(word_col_content);
        word_range_list.push(word_range_content);
        state_list.push(state.unwrap_or(WordType::EnNum));
    }

    let aligned = range_shape_ok
        && ranges.is_some()
        && word_range_list
            .iter()
            .zip(word_list.iter())
            .all(|(spans, words)| spans.len() == words.len());

    WordInfo {
        words: word_list,
        word_cols: word_col_list,
        word_types: state_list,
        char_cols: if aligned {
            word_range_list
        } else {
            // 退回旧估计法：清空区间表，并把降级记在账上，不留下一框"看起来像对齐结果"
            // 但没有对齐来源的几何。
            Vec::new()
        },
        align_status: if aligned {
            AlignStatus::Aligned
        } else {
            AlignStatus::Fallback
        },
        ..WordInfo::default()
    }
}

fn has_chinese_char(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
        || ('\u{3000}'..='\u{303f}').contains(&ch)
        || ('\u{ff00}'..='\u{ffef}').contains(&ch)
}

fn round5(v: f32) -> f32 {
    (v * 100_000.0).round_ties_even() / 100_000.0
}

fn round5_f64(v: f64) -> f32 {
    ((v * 100_000.0).round_ties_even() / 100_000.0) as f32
}

#[cfg(test)]
mod tests {
    use ndarray::Array3;

    use super::CtcLabelDecoder;
    use crate::ocr::types::AlignStatus;

    /// 字符表 `["a", "b"]` 装好后的完整类别：`[blank, a, b, " "]`（下标 0/1/2/3）。
    const CLASSES: usize = 4;
    const BLANK: usize = 0;
    const A: usize = 1;
    const B: usize = 2;
    const SPACE: usize = 3;

    fn decoder() -> CtcLabelDecoder {
        CtcLabelDecoder::new(Some(vec!["a".into(), "b".into()]), None)
            .expect("decoder init should pass")
    }

    /// 每帧一个 argmax 类别，其余类别均分剩余概率。
    fn peaked(frames: &[usize]) -> Array3<f32> {
        let peak = 0.9_f32;
        let off = (1.0 - peak) / (CLASSES - 1) as f32;
        let data: Vec<f32> = frames
            .iter()
            .flat_map(|&id| (0..CLASSES).map(move |c| if c == id { peak } else { off }))
            .collect();
        Array3::from_shape_vec((1, frames.len(), CLASSES), data).expect("shape matches")
    }

    #[test]
    fn decode_basic_ctc() {
        let decoder = decoder();
        // character list after init: [blank, a, b, " "]
        let preds = peaked(&[BLANK, A, A, B]);

        let (lines, words) = decoder
            .decode(&preds, true, &[1.0], 1.0)
            .expect("decode should pass");
        assert_eq!(lines[0].0, "ab");
        assert_eq!(words.len(), 1);
    }

    /// 对齐可用的基本盘：`char_cols` 与 `words` 逐层等长，区间有序且互不重叠。
    #[test]
    fn decode_emits_one_disjoint_column_range_per_character() {
        let decoder = decoder();
        let preds = peaked(&[BLANK, A, A, A, BLANK, B, B, BLANK]);

        let (_, words) = decoder
            .decode(&preds, true, &[1.0], 1.0)
            .expect("decode should pass");
        let info = &words[0];
        assert_eq!(info.align_status, AlignStatus::Aligned);
        assert_eq!(info.words.len(), info.char_cols.len());
        assert!(
            info.char_cols
                .iter()
                .zip(info.words.iter())
                .all(|(spans, ws)| spans.len() == ws.len()),
            "char_cols 必须与 words 逐层等长"
        );

        let flat: Vec<[usize; 2]> = info.char_cols.iter().flatten().copied().collect();
        assert_eq!(flat.len(), 2, "a 与 b 各一项");
        let mut previous_end = 0;
        for range in &flat {
            assert!(range[0] < range[1], "零宽区间: {range:?}");
            assert!(range[0] >= previous_end, "区间重叠或倒退: {flat:?}");
            assert!(range[1] <= preds.dim().1, "区间越出帧数: {flat:?}");
            previous_end = range[1];
        }
    }

    /// 重复字符专项：collapse 后的两个 `a` 在 argmax 路径上共用同一段峰，但对齐必须
    /// 把它们分到两个不相交的区间（这正是“连续非 blank argmax”做不到的）。
    #[test]
    fn decode_repeated_characters_get_disjoint_ranges() {
        let decoder = decoder();
        // argmax: blank, a, a, blank, a, a  -> collapse 后文本 "aa"
        let preds = peaked(&[BLANK, A, A, BLANK, A, A]);

        let (lines, words) = decoder
            .decode(&preds, true, &[1.0], 1.0)
            .expect("decode should pass");
        assert_eq!(lines[0].0, "aa");
        let info = &words[0];
        assert_eq!(info.align_status, AlignStatus::Aligned);
        let spans = &info.char_cols[0];
        assert_eq!(spans.len(), 2, "两个 a 必须各自拿到区间");
        assert!(spans[0][1] <= spans[1][0], "重复字符的区间相交: {spans:?}");
    }

    /// blank 与空格：空格字符本身不进门词表，但它的区间必须把两个词推开，
    /// 下游铺格才能把字缝分给两侧而不是抹平。
    #[test]
    fn decode_space_splits_words_but_keeps_its_own_range_out_of_char_cols() {
        let decoder = decoder();
        let preds = peaked(&[BLANK, A, A, SPACE, SPACE, B, B, BLANK]);

        let (lines, words) = decoder
            .decode(&preds, true, &[1.0], 1.0)
            .expect("decode should pass");
        assert_eq!(lines[0].0, "a b");
        let info = &words[0];
        assert_eq!(
            info.words,
            vec![vec!["a".to_string()], vec!["b".to_string()]]
        );
        assert_eq!(info.align_status, AlignStatus::Aligned);
        assert_eq!(info.char_cols.len(), 2);
        assert!(
            info.char_cols.iter().all(|spans| spans.len() == 1),
            "空格不该把自己的区间交给任何一个字: {:?}",
            info.char_cols
        );
        assert!(
            info.char_cols[0][0][1] < info.char_cols[1][0][0],
            "被空格隔开的两个字区间相接: {:?}",
            info.char_cols
        );
    }

    /// 对齐不可行时的记账：真实情形下 argmax 路径本身就是一条合法对齐路径，
    /// 因此能在解码入口上触发退回的是“字符表条目不是单个字符”——它把
    /// “一个字符 = 一个 token”这个下标约定打破，下游按字符编号的映射全部失效。
    /// 此时必须记为 `Fallback`，`char_cols` 不得留残留。
    #[test]
    fn decode_records_fallback_when_a_dict_entry_is_not_a_single_char() {
        // 字符表里只有 "ab" 这个双字条目：文本得到 2 个字符，但只有 1 个 token。
        let decoder =
            CtcLabelDecoder::new(Some(vec!["ab".into()]), None).expect("decoder init should pass");
        // character: [blank, "ab", " "]
        let preds = {
            let frames = [BLANK, A, A, BLANK];
            let peak = 0.9_f32;
            let off = (1.0 - peak) / 2.0;
            let data: Vec<f32> = frames
                .iter()
                .flat_map(|&id| (0..3).map(move |c| if c == id { peak } else { off }))
                .collect();
            Array3::from_shape_vec((1, frames.len(), 3), data).expect("shape matches")
        };

        let (lines, words) = decoder
            .decode(&preds, true, &[1.0], 1.0)
            .expect("decode should pass");
        assert_eq!(lines[0].0, "ab", "双字条目照旧拼进文本");
        let info = &words[0];
        assert_eq!(info.align_status, AlignStatus::Fallback);
        assert!(info.char_cols.is_empty(), "退回时不得留下来源不明的区间");
        assert!(!info.words.is_empty(), "退回不影响切词与文本");
    }

    #[test]
    fn decode_rejects_wh_ratio_len_mismatch_when_word_box_enabled() {
        let decoder = decoder();
        let preds = peaked(&[BLANK, A]);

        let err = decoder
            .decode(&preds, true, &[], 1.0)
            .expect_err("must reject mismatch");
        assert!(
            err.to_string()
                .contains("wh_ratio_list length 0 does not match batch size 1")
        );
    }
}
