//! CTC forced alignment：给已经解码出来的文本，逐字符求出它在时间轴上占用的列区间。
//!
//! 为什么不是"连续非 blank 的 argmax 区间"：argmax 路径上相邻两帧同字会被 collapse
//! 成一次，`ll` / `mm` / `1122` 这类重复字符因此**共用**一个列，后处理拿不到"第二个 l
//! 在哪"的信息。forced alignment 反过来做：文本是已知量，在 2L+1 状态图
//! （blank, c₁, blank, c₂, …, blank, c_L, blank）上取 Viterbi 最优路径，重复字符落在
//! **两个不同的状态**上，各自拿到属于自己的帧区间。
//!
//! 输出的是 CTC 列（帧）区间，半开 `[start, end)`，不做任何像素换算 —— 像素映射与
//! 邻字边界归属都归 [`word_boxes`] 管，本模块只回答"这个字占哪几帧"。

use ndarray::{ArrayView2, Axis};

/// blank 在字符表里的固定下标（`CtcLabelDecoder::new` 把 `"blank"` 插在 0 号位）。
const BLANK_ID: usize = 0;

/// 概率下限。模型确实会把某些帧的概率压到 0（下溢），`ln(0) = -inf` 会让整条前向路径
/// 变成 `-inf` 而失去可比性。1e-30 比任何真实 emission 低若干个数量级，只用来兜底，
/// 不参与任何判定。
const MIN_PROB: f32 = 1e-30;

/// 逐字符强制对齐。
///
/// * `probs` — 这一行的 CTC 输出，形状 `T × C`（**完整逐步概率**，不是 argmax 之后的序列）。
/// * `labels` — 目标文本逐字符的 token id（不含 blank，顺序即文本顺序）。
///
/// 返回 `Some(ranges)`，`ranges[i] = [first_frame, last_frame + 1)`，与 `labels` 等长；
/// 返回 `None` 表示**这次对齐不可行**，调用方必须退回原来的估计法并记账：
/// 帧数装不下标签序列（连每字符一帧都排不开）、类别维为 0、标签 id 越界，
/// 或者回溯路径没能覆盖到每一个字符。
pub(crate) fn char_column_ranges(
    probs: ArrayView2<'_, f32>,
    labels: &[usize],
) -> Option<Vec<[usize; 2]>> {
    let (timesteps, class_num) = probs.dim();
    let len = labels.len();

    if len == 0 {
        return Some(Vec::new());
    }
    if class_num == 0 {
        return None;
    }
    // 状态图上每帧至少能推进一步（相邻的不同字符可以跳过中间那个 blank 状态），
    // 所以理论下限就是"每字符一帧"。比这还短的行拿不到可用的区间，直接判不可行。
    if timesteps < len {
        return None;
    }
    if labels.iter().any(|&id| id >= class_num) {
        return None;
    }

    let states = 2 * len + 1;
    let neg_inf = f32::NEG_INFINITY;

    // emission 先按 (帧, 字符) 取出来做一次 ln：DP 循环里就只剩加法与比较。
    let mut blank_log = vec![0.0_f32; timesteps];
    let mut label_log = vec![0.0_f32; len * timesteps];
    for (t, row) in probs.axis_iter(Axis(0)).enumerate() {
        blank_log[t] = ln_clamped(row[BLANK_ID]);
        for (i, &id) in labels.iter().enumerate() {
            label_log[i * timesteps + t] = ln_clamped(row[id]);
        }
    }

    // 奇数状态（字符）之间的可达性预计算：相邻两个字符**不同**时允许跳过中间那个
    // blank 状态（CTC 的 collapse 本来就会把它们收成两个字符）。不允许这个跳跃的话，
    // 每对相邻字符都要吃掉一帧 blank，长行的帧预算会被白白抽走。
    // 相邻两个字符**相同**时必须走 blank，否则 collapse 会把它们并成一个。
    let mut jump_from = vec![false; states];
    for j in (1..states).step_by(2) {
        let prev = j / 2;
        jump_from[j] = prev > 0 && labels[prev] != labels[prev - 1];
    }

    let mut delta = vec![neg_inf; states];
    let mut next_delta = vec![neg_inf; states];
    let mut emit = vec![0.0_f32; states];
    // back[t * states + j]：t 帧处于状态 j 时，上一帧在 j（0）、j-1（1）还是 j-2（2）。
    let mut back = vec![0u8; timesteps * states];

    for t in 0..timesteps {
        for (j, e) in emit.iter_mut().enumerate() {
            *e = if j % 2 == 0 {
                blank_log[t]
            } else {
                label_log[(j / 2) * timesteps + t]
            };
        }

        if t == 0 {
            // 起点：空串前缀（state 0）或第一个字符（state 1）。其余状态此刻不可达。
            delta[0] = emit[0];
            delta[1] = emit[1];
            delta[2..].fill(neg_inf);
            continue;
        }

        for j in 0..states {
            let mut best = delta[j];
            let mut from = 0u8;
            if j > 0 && delta[j - 1] > best {
                best = delta[j - 1];
                from = 1;
            }
            if j > 1 && jump_from[j] && delta[j - 2] > best {
                best = delta[j - 2];
                from = 2;
            }
            next_delta[j] = if best == neg_inf {
                neg_inf
            } else {
                best + emit[j]
            };
            back[t * states + j] = from;
        }
        std::mem::swap(&mut delta, &mut next_delta);
    }

    // 终点允许停在末字符（2L-1）或收尾 blank（2L）：后者是常规情形，前者处理文本
    // 刚好铺满帧数的长行。两者都不可达时（帧预算不够）这里拿到的是 -inf 平票，
    // 交给下面的覆盖自检处理。
    let mut j = if delta[states - 1] >= delta[states - 2] {
        states - 1
    } else {
        states - 2
    };

    let mut ranges = vec![[usize::MAX, 0usize]; len];
    for t in (0..timesteps).rev() {
        if j % 2 == 1 {
            let i = j / 2;
            // 倒着走：起点每见一次就覆盖（最后留下最小的 t），终点只留第一次（最大的 t）。
            ranges[i][0] = t;
            if ranges[i][1] == 0 {
                ranges[i][1] = t + 1;
            }
        }
        j -= back[t * states + j] as usize;
    }

    // 兜底自检：状态图保证每个字符状态都被踩过，若不然说明上面的 DP 写错了，
    // 此时交出一个错误的区间表比退回估计法更糟。
    let mut previous_end = 0usize;
    for range in &ranges {
        if range[0] == usize::MAX
            || range[1] == 0
            || range[0] >= range[1]
            || range[0] < previous_end
        {
            return None;
        }
        previous_end = range[1];
    }

    Some(ranges)
}

fn ln_clamped(prob: f32) -> f32 {
    prob.max(MIN_PROB).ln()
}

#[cfg(test)]
mod tests {
    use ndarray::Array2;

    use super::char_column_ranges;

    /// 便捷构造：`frames` 里每一项是一帧的 argmax 类别（其余类别给 `off` 概率）。
    /// 类别 0 是 blank。
    fn peaked(frames: &[usize], class_num: usize, peak: f32) -> Array2<f32> {
        let off = (1.0 - peak) / (class_num - 1) as f32;
        let data: Vec<f32> = frames
            .iter()
            .flat_map(|&id| (0..class_num).map(move |c| if c == id { peak } else { off }))
            .collect();
        Array2::from_shape_vec((frames.len(), class_num), data).expect("shape matches")
    }

    #[test]
    fn aligns_repeated_characters_to_separate_ranges() {
        // 字符表 [blank, a, b]；argmax 路径 "a a a b b" collapse 后只有一个 a，
        // 但对齐目标 "aa" 必须拿到两段互不相交的区间。
        let probs = peaked(&[0, 1, 1, 1, 2, 2, 0], 3, 0.9);
        let view = probs.view();
        let ranges = char_column_ranges(view, &[1, 1, 2]).expect("alignment feasible");
        assert_eq!(ranges.len(), 3);
        assert!(
            ranges[0][1] <= ranges[1][0],
            "两个 a 必须分开：{:?} / {:?}",
            ranges[0],
            ranges[1]
        );
        assert!(ranges[0][0] >= 1 && ranges[0][1] <= 4, "{:?}", ranges[0]);
        assert!(ranges[1][0] >= 1 && ranges[1][1] <= 5, "{:?}", ranges[1]);
        assert_eq!(ranges[2], [4, 6]);
    }

    #[test]
    fn aligns_when_frame_budget_is_exactly_used_up() {
        // "a b a b" 需要 8 个状态位置；这里给 L=2、T=4（= 2L），帧数一帧都富余不出来，
        // 路径仍然必须走通，每个字符各占一帧起。
        let probs = peaked(&[1, 0, 2, 2], 3, 0.9);
        let ranges = char_column_ranges(probs.view(), &[1, 2]).expect("exactly enough frames");
        assert_eq!(ranges, vec![[0, 1], [2, 4]]);
    }

    #[test]
    fn rejects_when_frames_cannot_hold_the_labels() {
        // 4 个字符至少需要 4 帧（不同字符可以跳过 blank 状态），这里只给了 3 帧
        // -> 不可行，调用方据此退回估计法。
        let probs = peaked(&[1, 0, 2], 3, 0.9);
        assert!(char_column_ranges(probs.view(), &[1, 2, 1, 2]).is_none());
    }

    /// 相邻的**不同**字符不需要一帧 blank 隔开：CTC 的 collapse 本来就会把它们
    /// 收成两个字符。不允许这个跳跃的话，每对相邻字符都要白吃一帧，长行的
    /// 帧预算会被抽走。
    #[test]
    fn distinct_adjacent_labels_need_no_blank_between_them() {
        let probs = peaked(&[1, 2], 3, 0.9);
        let ranges = char_column_ranges(probs.view(), &[1, 2]).expect("two frames for two labels");
        assert_eq!(ranges, vec![[0, 1], [1, 2]]);
    }

    /// 相邻的**相同**字符必须有 blank 隔开，否则 collapse 会把它们并成一个：
    /// 同样的两帧，重复字符就是不可行（而旧估计法会把 `ll` 挤成一格平均宽）。
    #[test]
    fn identical_adjacent_labels_cannot_share_the_frame_budget() {
        let probs = peaked(&[1, 1], 3, 0.9);
        assert!(char_column_ranges(probs.view(), &[1, 1]).is_none());
        // 多给一帧 blank 就够了，而且两个字符必须各自拿到区间。
        let probs = peaked(&[1, 0, 1], 3, 0.9);
        let ranges = char_column_ranges(probs.view(), &[1, 1]).expect("three frames for a-a");
        assert_eq!(ranges, vec![[0, 1], [2, 3]]);
    }

    #[test]
    fn rejects_out_of_range_label_id() {
        let probs = peaked(&[1, 1, 2, 2], 3, 0.9);
        assert!(char_column_ranges(probs.view(), &[7]).is_none());
    }

    #[test]
    fn empty_text_aligns_to_empty_ranges() {
        let probs = peaked(&[0, 0], 3, 0.9);
        let ranges: Vec<[usize; 2]> = char_column_ranges(probs.view(), &[]).expect("empty");
        assert!(ranges.is_empty());
    }

    #[test]
    fn wide_character_earns_more_frames_than_narrow_one() {
        // 手工摆一帧概率：'i'（类别 1）只亮两帧，'m'（类别 2）亮五帧。
        // forced alignment 因此给出不等宽的区间，这是"整行平均字宽"给不了的形状。
        let mut probs = Array2::<f32>::from_elem((9, 3), 0.02);
        let bright = [
            (0usize, 0usize),
            (1, 1),
            (2, 1),
            (3, 2),
            (4, 2),
            (5, 2),
            (6, 2),
            (7, 2),
            (8, 0),
        ];
        for (t, id) in bright {
            probs[[t, id]] = 0.9;
        }
        let ranges = char_column_ranges(probs.view(), &[1, 2]).expect("alignment feasible");
        let w_i = ranges[0][1] - ranges[0][0];
        let w_m = ranges[1][1] - ranges[1][0];
        assert_eq!(ranges[0], [1, 3]);
        assert_eq!(ranges[1], [3, 8]);
        assert!(
            w_m > w_i,
            "窄字 'i' 与宽字 'm' 拿到了同样宽度（{w_i} vs {w_m}），对齐没有用上概率质量"
        );
    }
}
