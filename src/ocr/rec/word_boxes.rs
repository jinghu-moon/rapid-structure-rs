use nalgebra::{SMatrix, SVector};

use crate::{
    Quad,
    config::RecImage,
    error::{RapidOcrError, Result},
    ocr::types::{AlignStatus, LineResult, WordBox, WordInfo, WordType},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Horizontal,
    Vertical,
}

pub fn compute_word_boxes(
    imgs: &[RecImage],
    dt_boxes: &[Quad],
    lines: &[LineResult],
    return_single_char_box: bool,
) -> Result<Vec<Vec<WordBox>>> {
    if imgs.len() != dt_boxes.len() || imgs.len() != lines.len() {
        return Err(RapidOcrError::InvalidInput(format!(
            "compute_word_boxes input length mismatch: imgs={}, dt_boxes={}, lines={}",
            imgs.len(),
            dt_boxes.len(),
            lines.len()
        )));
    }

    Ok(compute_word_boxes_unchecked(
        imgs,
        dt_boxes,
        lines,
        return_single_char_box,
    ))
}

fn compute_word_boxes_unchecked(
    imgs: &[RecImage],
    dt_boxes: &[Quad],
    lines: &[LineResult],
    return_single_char_box: bool,
) -> Vec<Vec<WordBox>> {
    let mut out = Vec::new();

    for ((img, det_box), line) in imgs.iter().zip(dt_boxes.iter()).zip(lines.iter()) {
        let Some(word_info) = &line.word_info else {
            out.push(Vec::new());
            continue;
        };

        if line.text.is_empty() || word_info.line_txt_len == 0.0 {
            out.push(Vec::new());
            continue;
        }

        let img_box = [
            [0.0_f32, 0.0_f32],
            [img.width() as f32, 0.0_f32],
            [img.width() as f32, img.height() as f32],
            [0.0_f32, img.height() as f32],
        ];

        let (word_content, mut word_boxes, confs) =
            cal_ocr_word_box(&line.text, img_box, word_info, return_single_char_box);

        adjust_box_overlap(&mut word_boxes);
        let direction = get_box_direction(*det_box);
        let mapped = reverse_rotate_crop_image(*det_box, &word_boxes, direction);

        let item = word_content
            .into_iter()
            .zip(confs.into_iter().chain(std::iter::repeat(0.0)))
            .zip(mapped)
            .map(|((text, score), bbox)| WordBox { text, score, bbox })
            .collect::<Vec<_>>();

        out.push(item);
    }

    out
}

fn get_box_direction(box_: Quad) -> Direction {
    let edge_lengths = [
        l2(box_[0], box_[1]),
        l2(box_[1], box_[2]),
        l2(box_[2], box_[3]),
        l2(box_[3], box_[0]),
    ];

    let width = edge_lengths[0].max(edge_lengths[2]);
    let height = edge_lengths[1].max(edge_lengths[3]);

    if width < 1e-6 {
        return Direction::Vertical;
    }

    let aspect_ratio = (height / width * 100.0).round() / 100.0;
    if aspect_ratio >= 1.5 {
        Direction::Vertical
    } else {
        Direction::Horizontal
    }
}

fn cal_ocr_word_box(
    rec_txt: &str,
    bbox: Quad,
    word_info: &WordInfo,
    return_single_char_box: bool,
) -> (Vec<String>, Vec<Quad>, Vec<f32>) {
    if rec_txt.is_empty() || word_info.line_txt_len == 0.0 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let (x0, y0, x1, y1) = quad_to_rect_bbox(&bbox);
    let avg_col_width = (x1 - x0) / word_info.line_txt_len;

    let is_all_en_num = word_info
        .word_types
        .iter()
        .all(|v| matches!(v, WordType::EnNum));
    let merge_en_num_words = is_all_en_num && !return_single_char_box;

    let mut line_cols: Vec<Vec<usize>> = Vec::new();
    let mut char_widths = Vec::new();
    let mut word_contents = Vec::new();

    for (word, word_col) in word_info.words.iter().zip(word_info.word_cols.iter()) {
        if merge_en_num_words {
            line_cols.push(word_col.clone());
            word_contents.push(word.join(""));
        } else {
            for col in word_col {
                line_cols.push(vec![*col]);
            }
            for token in word {
                word_contents.push(token.clone());
            }
        }

        if word_col.len() > 1 {
            char_widths.push(calc_avg_char_width(word_col, avg_col_width));
        }
    }

    // 几何优先用 forced alignment 的逐字符列区间铺格；拿不到（记账为 `Fallback`，
    // 或区间表与 `words` 的形状对不上）才退回“列索引 + 整行平均字宽”的旧估计法。
    // 两条路径产出的框数量与 `word_contents` 同步，因此输出面形状不随算法改变。
    let boxes = match aligned_segments(word_info, merge_en_num_words) {
        Some(spans) => tile_column_spans(
            &spans,
            word_info.line_txt_len,
            avg_col_width,
            (x0, y0, x1, y1),
        ),
        None if merge_en_num_words => {
            let avg_char_width =
                calc_all_char_avg_width(&char_widths, x0, x1, rec_txt.chars().count());
            calc_en_num_box(&line_cols, avg_char_width, avg_col_width, (x0, y0, x1, y1))
        }
        None => {
            let avg_char_width =
                calc_all_char_avg_width(&char_widths, x0, x1, rec_txt.chars().count());
            line_cols
                .iter()
                .flat_map(|cols| calc_box(cols, avg_char_width, avg_col_width, (x0, y0, x1, y1)))
                .collect::<Vec<_>>()
        }
    };

    (word_contents, boxes, word_info.confs.clone())
}

/// 把 `char_cols` 里的逐字符列区间折算成**输出粒度**的区间表：字档一项一字，
/// 纯英数词档一项一词（整词区间 = 首字起点..末字终点）。
///
/// 返回 `None` 表示这次用不了对齐结果，几何必须退回估计法。这里刻意不做任何
/// “就近补一格”的救火：形状不对就是不对，宁可退回旧算法并让降级留在
/// `align_status` 的账上，也不交出一个来源不明的框。
fn aligned_segments(word_info: &WordInfo, merge_words: bool) -> Option<Vec<[f32; 2]>> {
    if !matches!(word_info.align_status, AlignStatus::Aligned) {
        return None;
    }
    if word_info.char_cols.len() != word_info.words.len() {
        return None;
    }

    let mut segments = Vec::new();
    for spans in &word_info.char_cols {
        let first = spans.first()?;
        let last = spans.last()?;
        if merge_words {
            segments.push([first[0] as f32, last[1] as f32]);
        } else {
            segments.extend(spans.iter().map(|span| [span[0] as f32, span[1] as f32]));
        }
    }

    (!segments.is_empty()).then_some(segments)
}

/// 把列区间铺成互不重叠、且把内容宽度分完的**列边界**，再乘每列像素宽落回裁剪图坐标。
///
/// 规则只有两条，而且彼此对称：
/// * 相邻区间之间那段 blank（字缝）取中点平分；
/// * 行首行尾的外扩只取“本行实测字缝中位宽度之半”，而不是拉到裁剪图两端。
///
/// 外边界那一条是量出来的，不是审美：检测框经常比文本宽，而 Viterbi 路径在行尾
/// 就离开了最后一个字符状态，“取到内容右端的中点”会把整段行尾空白全量给末字
/// （`[实测]` 首轮对齐报告里末字宽达 GT 的 2–5 倍，`width_ratio.max` 5.43）。
/// 没有实测字缝（只有一个区间，或等宽方块字根本不留缝）时外扩为 0，末边界就是区间端点。
///
/// 这一步不做“平均字宽”的再分配：比例字体下 `i` 与 `m` 占的帧数本来就不同，把它们
/// 拉平正是旧估计法的系统性偏差来源（docs/13 §4.5）。
fn tile_column_spans(
    spans: &[[f32; 2]],
    content_cols: f32,
    avg_col_width: f32,
    bbox_points: (f32, f32, f32, f32),
) -> Vec<Quad> {
    let (x0, y0, x1, y1) = bbox_points;
    let outer_pad = median_positive_gap(spans) / 2.0;

    let mut bounds: Vec<f32> = Vec::with_capacity(spans.len() + 1);
    bounds.push(spans[0][0] - outer_pad);
    for pair in spans.windows(2) {
        bounds.push((pair[0][1] + pair[1][0]) / 2.0);
    }
    bounds.push(spans[spans.len() - 1][1] + outer_pad);

    for bound in &mut bounds {
        *bound = bound.clamp(0.0, content_cols);
    }
    // 区间表来自单调的 Viterbi 路径，正常情况不会倒退；倒退只在 clamp 把越界
    // 区间压回内容右端时出现，此时退化成零宽格子，由下面的 1 px 护栏兜住。
    for i in 1..bounds.len() {
        bounds[i] = bounds[i].max(bounds[i - 1]);
    }

    let mut results = Vec::with_capacity(bounds.len() - 1);
    for pair in bounds.windows(2) {
        let left = (pair[0] * avg_col_width).floor() + x0;
        let right = (pair[1] * avg_col_width).floor() + x0;
        let right = if right <= left {
            (left + 1.0).min(x1)
        } else {
            right.min(x1)
        };
        results.push([[left, y0], [right, y0], [right, y1], [left, y1]]);
    }

    results
}

/// 相邻区间之间那段 blank 的**中位**宽度（帧）。零宽字缝（等宽方块字不留缝、以及
/// Viterbi 直接跳跃的情形）不计入中位数；没有字缝可统计时返回 0。
fn median_positive_gap(spans: &[[f32; 2]]) -> f32 {
    let mut gaps: Vec<f32> = spans
        .windows(2)
        .map(|pair| (pair[1][0] - pair[0][1]).max(0.0))
        .filter(|gap| *gap > 0.0)
        .collect();
    if gaps.is_empty() {
        return 0.0;
    }
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = gaps.len() / 2;
    if gaps.len() % 2 == 1 {
        gaps[mid]
    } else {
        (gaps[mid - 1] + gaps[mid]) / 2.0
    }
}

fn calc_en_num_box(
    line_cols: &[Vec<usize>],
    avg_char_width: f32,
    avg_col_width: f32,
    bbox_points: (f32, f32, f32, f32),
) -> Vec<Quad> {
    let mut results = Vec::new();
    for cols in line_cols {
        let boxes = calc_box(cols, avg_char_width, avg_col_width, bbox_points);
        if boxes.is_empty() {
            continue;
        }
        let (x0, y0, x1, y1) = quad_vec_to_rect_bbox(&boxes);
        results.push([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
    }
    results
}

fn calc_box(
    cols: &[usize],
    avg_char_width: f32,
    avg_col_width: f32,
    bbox_points: (f32, f32, f32, f32),
) -> Vec<Quad> {
    let (x0, y0, x1, y1) = bbox_points;
    let mut results = Vec::new();

    for col_idx in cols {
        let center_x = (*col_idx as f32 + 0.5) * avg_col_width;

        let char_x0 = ((center_x - avg_char_width / 2.0).max(0.0)).floor() + x0;
        let char_x1 = ((center_x + avg_char_width / 2.0).min(x1 - x0)).floor() + x0;

        results.push([[char_x0, y0], [char_x1, y0], [char_x1, y1], [char_x0, y1]]);
    }

    results.sort_by(|a, b| {
        a[0][0]
            .partial_cmp(&b[0][0])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

fn calc_avg_char_width(word_col: &[usize], each_col_width: f32) -> f32 {
    let total = (word_col[word_col.len() - 1] - word_col[0]) as f32 * each_col_width;
    total / (word_col.len() as f32 - 1.0)
}

fn calc_all_char_avg_width(width_list: &[f32], bbox_x0: f32, bbox_x1: f32, txt_len: usize) -> f32 {
    if txt_len == 0 {
        return 0.0;
    }
    if !width_list.is_empty() {
        return width_list.iter().sum::<f32>() / width_list.len() as f32;
    }
    (bbox_x1 - bbox_x0) / txt_len as f32
}

fn adjust_box_overlap(word_box_list: &mut [Quad]) {
    if word_box_list.len() < 2 {
        return;
    }

    for i in 0..(word_box_list.len() - 1) {
        let cur_right = word_box_list[i][1][0];
        let next_left = word_box_list[i + 1][0][0];

        if cur_right > next_left {
            let distance = (cur_right - next_left).abs();
            word_box_list[i][1][0] -= distance / 2.0;
            word_box_list[i][2][0] -= distance / 2.0;
            word_box_list[i + 1][0][0] += distance / 2.0;
            word_box_list[i + 1][3][0] += distance / 2.0;
        }
    }
}

fn reverse_rotate_crop_image(
    bbox_points: Quad,
    word_points_list: &[Quad],
    direction: Direction,
) -> Vec<Quad> {
    let left = bbox_points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, |acc, v| acc.min(v));
    let top = bbox_points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, |acc, v| acc.min(v));

    let mut local_bbox = bbox_points;
    for p in &mut local_bbox {
        p[0] -= left;
        p[1] -= top;
    }

    let img_crop_width = l2(local_bbox[0], local_bbox[1]);
    let img_crop_height = l2(local_bbox[0], local_bbox[3]);

    let pts_std = [
        [0.0_f32, 0.0_f32],
        [img_crop_width, 0.0_f32],
        [img_crop_width, img_crop_height],
        [0.0_f32, img_crop_height],
    ];

    let h = homography_from_4pt(local_bbox, pts_std);
    let inv = h
        .try_inverse()
        .unwrap_or_else(SMatrix::<f32, 3, 3>::identity);

    let mut out = Vec::with_capacity(word_points_list.len());
    for word_points in word_points_list {
        let mut mapped = [[0.0_f32; 2]; 4];

        for (i, point) in word_points.iter().enumerate() {
            let mut p = *point;
            if matches!(direction, Direction::Vertical) {
                p = s_rotate(-90.0_f32.to_radians(), p[0], p[1], 0.0, 0.0);
                p[0] += img_crop_width;
            }

            let v = inv * SVector::<f32, 3>::new(p[0], p[1], 1.0);
            let x = if v[2] != 0.0 { v[0] / v[2] } else { v[0] };
            let y = if v[2] != 0.0 { v[1] / v[2] } else { v[1] };

            mapped[i] = [x + left, y + top];
        }

        out.push(order_points(mapped));
    }

    out
}

fn homography_from_4pt(src: Quad, dst: Quad) -> SMatrix<f32, 3, 3> {
    let mut a = SMatrix::<f32, 8, 8>::zeros();
    let mut b = SVector::<f32, 8>::zeros();

    for i in 0..4 {
        let x = src[i][0];
        let y = src[i][1];
        let x_cap = dst[i][0];
        let y_cap = dst[i][1];

        let r0 = i * 2;
        let r1 = r0 + 1;

        a[(r0, 0)] = x;
        a[(r0, 1)] = y;
        a[(r0, 2)] = 1.0;
        a[(r0, 6)] = -x * x_cap;
        a[(r0, 7)] = -y * x_cap;
        b[r0] = x_cap;

        a[(r1, 3)] = x;
        a[(r1, 4)] = y;
        a[(r1, 5)] = 1.0;
        a[(r1, 6)] = -x * y_cap;
        a[(r1, 7)] = -y * y_cap;
        b[r1] = y_cap;
    }

    if let Some(h) = a.lu().solve(&b) {
        SMatrix::<f32, 3, 3>::from_row_slice(&[h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
    } else {
        SMatrix::<f32, 3, 3>::identity()
    }
}

fn s_rotate(angle: f32, valuex: f32, valuey: f32, pointx: f32, pointy: f32) -> [f32; 2] {
    let x = (valuex - pointx) * angle.cos() + (valuey - pointy) * angle.sin() + pointx;
    let y = (valuey - pointy) * angle.cos() - (valuex - pointx) * angle.sin() + pointy;
    [x, y]
}

fn order_points(ori_box: Quad) -> Quad {
    let mut points = ori_box.to_vec();
    points.sort_by(|a, b| {
        a[1].partial_cmp(&b[1])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut top = [points[0], points[1]];
    let mut bottom = [points[2], points[3]];
    top.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    bottom.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));

    [top[0], top[1], bottom[1], bottom[0]]
}

fn quad_to_rect_bbox(quad: &Quad) -> (f32, f32, f32, f32) {
    let b = crate::vision::geometry::aabb_from_quad(quad);
    (b[0], b[1], b[2], b[3])
}

fn quad_vec_to_rect_bbox(quads: &[Quad]) -> (f32, f32, f32, f32) {
    let mut x_min = f32::INFINITY;
    let mut y_min = f32::INFINITY;
    let mut x_max = f32::NEG_INFINITY;
    let mut y_max = f32::NEG_INFINITY;

    for quad in quads {
        for point in quad {
            x_min = x_min.min(point[0]);
            y_min = y_min.min(point[1]);
            x_max = x_max.max(point[0]);
            y_max = y_max.max(point[1]);
        }
    }

    (x_min, y_min, x_max, y_max)
}

fn l2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use crate::{
        Quad,
        config::RecImage,
        error::RapidOcrError,
        ocr::types::{AlignStatus, LineResult, WordInfo, WordType},
    };

    use super::compute_word_boxes;

    const FLAT: [Quad; 1] = [[[0.0, 0.0], [100.0, 0.0], [100.0, 20.0], [0.0, 20.0]]];

    fn word_info(
        words: &[&str],
        word_cols: &[usize],
        line_txt_len: f32,
        char_cols: Vec<Vec<[usize; 2]>>,
        status: AlignStatus,
    ) -> WordInfo {
        WordInfo {
            words: vec![words.iter().map(|w| (*w).to_string()).collect()],
            word_cols: vec![word_cols.to_vec()],
            word_types: vec![WordType::EnNum],
            line_txt_len,
            confs: words.iter().map(|_| 0.9_f32).collect(),
            char_cols,
            align_status: status,
        }
    }

    fn left_x(box_: &Quad) -> f32 {
        box_[0][0]
    }

    fn right_x(box_: &Quad) -> f32 {
        box_[1][0]
    }

    fn assert_close(actual: f32, expected: f32, tag: &str) {
        assert!(
            (actual - expected).abs() < 0.05,
            "{tag}: expected {expected}, got {actual}"
        );
    }

    #[test]
    fn compute_word_boxes_smoke() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B"],
                &[1, 2],
                4.0,
                Vec::new(),
                AlignStatus::Empty,
            )),
        };
        let out =
            compute_word_boxes(&[img], &FLAT, &[line], false).expect("word boxes should compute");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 1);
        assert_eq!(out[0][0].text, "AB");
    }

    #[test]
    fn compute_word_boxes_rejects_mismatch() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: None,
        };

        let err = compute_word_boxes(&[img], &FLAT, &[line.clone(), line], false)
            .expect_err("should reject mismatched lengths");
        assert!(matches!(err, RapidOcrError::InvalidInput(_)));
    }

    /// 对齐可用时，逐字框必须把行宽**分完**：相邻框共用一条边界（不重叠也不留缝），
    /// 两端的空白各只取一半。这是呈现层“一次拖拽只选中一个字”的几何前提。
    #[test]
    fn aligned_char_boxes_tile_the_line_without_gaps() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        // 列区间 A=[0,1) B=[2,4)，行内容宽 4 列 -> 每列 25 px；边界落在 0 / 1.5 / 4。
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B"],
                &[1, 2],
                4.0,
                vec![vec![[0, 1], [2, 4]]],
                AlignStatus::Aligned,
            )),
        };

        let out = compute_word_boxes(&[img], &FLAT, &[line], true).expect("aligned boxes");
        assert_eq!(out[0].len(), 2);
        assert_close(left_x(&out[0][0].bbox), 0.0, "A 左边落在首区间左半空白中点");
        assert_close(right_x(&out[0][0].bbox), 37.0, "A 右边 = 字缝中点 1.5 列");
        assert_close(left_x(&out[0][1].bbox), 37.0, "B 左边与 A 右边同一条缝");
        assert_close(
            right_x(&out[0][1].bbox),
            100.0,
            "B 右边 = 末区间与内容右端的中点",
        );
    }

    /// 窄字与宽字拿到不同宽度：这是“整行平均字宽”给不了的形状，也是本步要修的
    /// 那条系统性偏差（docs/13 §4.5 第 5 条）的直接判据。
    #[test]
    fn aligned_char_boxes_follow_proportional_frame_widths() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        // 'i' 占 1 帧，'m' 占 6 帧；行宽 10 列 -> 每列 10 px。
        let line = LineResult {
            text: "iM".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["i", "M"],
                &[1, 3],
                10.0,
                vec![vec![[1, 2], [3, 9]]],
                AlignStatus::Aligned,
            )),
        };

        let out = compute_word_boxes(&[img], &FLAT, &[line], true).expect("aligned boxes");
        let width_i = right_x(&out[0][0].bbox) - left_x(&out[0][0].bbox);
        let width_m = right_x(&out[0][1].bbox) - left_x(&out[0][1].bbox);
        assert_close(left_x(&out[0][0].bbox), 5.0, "i 左边");
        assert_close(width_m / width_i, 3.5, "宽字/窄字 宽度比 (70/20)");
        assert!(width_m > width_i * 2.0, "对齐没有用上概率质量");
    }

    /// 记账为 `Fallback` 时，几何必须逐位退回旧估计法（列中心 ± 平均字宽），
    /// 而不是静默产出一批来源不明的框。
    #[test]
    fn fallback_status_keeps_the_average_width_estimator() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        // 同样的列与行宽，但 align_status = Fallback：期望值就是改造前的算式结果
        // （列 1 中心 37.5 ± 12.5 -> [25,50]；列 2 中心 62.5 ± 12.5 -> [50,75]）。
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B"],
                &[1, 2],
                4.0,
                Vec::new(),
                AlignStatus::Fallback,
            )),
        };

        let out = compute_word_boxes(&[img], &FLAT, &[line], true).expect("fallback boxes");
        assert_close(left_x(&out[0][0].bbox), 25.0, "fallback A 左边");
        assert_close(right_x(&out[0][0].bbox), 50.0, "fallback A 右边");
        assert_close(left_x(&out[0][1].bbox), 50.0, "fallback B 左边");
        assert_close(right_x(&out[0][1].bbox), 75.0, "fallback B 右边");
    }

    /// 词档（纯英数不拆字）下，对齐区间聚合到整词：一个词一项，区间 = 首字起点..末字终点。
    /// 聚合后只有一个区间，没有字缝可统计，因此外扩为 0：整词框就是区间本身，
    /// **不拿行尾空白充门面**（单框行情下“取到内容右端的中点”会把框胖出一倍）。
    #[test]
    fn aligned_word_mode_box_spans_the_whole_word() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B"],
                &[2, 5],
                10.0,
                vec![vec![[2, 4], [5, 7]]],
                AlignStatus::Aligned,
            )),
        };

        let out = compute_word_boxes(&[img], &FLAT, &[line], false).expect("word box");
        assert_eq!(out[0].len(), 1);
        assert_eq!(out[0][0].text, "AB");
        assert_close(left_x(&out[0][0].bbox), 20.0, "整词左边");
        assert_close(right_x(&out[0][0].bbox), 70.0, "整词右边");
    }

    /// 行尾留白不得全量给末字：检测框比文本宽时，末字的右边只能比自己的区间外扩
    /// **半个实测字缝**。这一条直接对着首轮对齐报告的 `width_ratio.max = 5.43`。
    #[test]
    fn trailing_crop_whitespace_is_not_handed_to_the_last_character() {
        // 裁剪图 200 px / 内容 20 列（每列 10 px），但文本只占到第 9 列：
        // 后面 11 列（110 px）全是检测框留白。字缝均为 1 列，外扩只给 0.5 列。
        let img = RecImage::from_bgr_u8(200, 20, vec![0; 200 * 20 * 3]).expect("valid image");
        let wide = [[[0.0_f32, 0.0_f32], [200.0, 0.0], [200.0, 20.0], [0.0, 20.0]]];
        let line = LineResult {
            text: "ABC".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B", "C"],
                &[1, 4, 7],
                20.0,
                vec![vec![[1, 3], [4, 6], [7, 9]]],
                AlignStatus::Aligned,
            )),
        };

        let out = compute_word_boxes(&[img], &wide, &[line], true).expect("aligned boxes");
        assert_eq!(out[0].len(), 3);
        // 旧规则会把末框拉到 (9 + 20) / 2 = 14.5 列 = 145 px（宽 90 px，是前两框的 3 倍）。
        assert_close(right_x(&out[0][2].bbox), 95.0, "末字右边只外扩半格字缝");
        assert_close(left_x(&out[0][0].bbox), 5.0, "首字左边同样只外扩半格");
        let last_width = right_x(&out[0][2].bbox) - left_x(&out[0][2].bbox);
        let first_width = right_x(&out[0][0].bbox) - left_x(&out[0][0].bbox);
        assert!(
            last_width / first_width < 1.2,
            "末字比首字宽了 {last_width}/{first_width}，行尾留白不该让给一个字"
        );
    }

    /// 竖排：列区间先落在识别裁剪图的横轴，再经单应（含轴交换）映回原图，
    /// 逐字框必须沿原图的**纵轴**按阅读序排开。
    #[test]
    fn aligned_char_boxes_land_on_the_original_image_for_vertical_lines() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        let det = [[[0.0, 0.0], [20.0, 0.0], [20.0, 100.0], [0.0, 100.0]]];
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B"],
                &[1, 2],
                4.0,
                vec![vec![[0, 1], [2, 4]]],
                AlignStatus::Aligned,
            )),
        };

        let out = compute_word_boxes(&[img], &det, &[line], true).expect("vertical boxes");
        // 裁剪图的 x∈[0,37] / [37,100] 经轴交换后变成原图的 y∈[0,37] / [37,100]。
        let spans = [(0usize, 0.0_f32, 37.0_f32), (1, 37.0, 100.0)];
        for (index, top, bottom) in spans {
            let box_ = &out[0][index].bbox;
            assert_close(box_[0][0], 0.0, "竖排左边 x");
            assert_close(box_[1][0], 20.0, "竖排右边 x");
            assert_close(box_[0][1], top, "竖排上边 y");
            assert_close(box_[2][1], bottom, "竖排下边 y");
        }
    }

    /// 倾斜四边形不得被替换成轴对齐外接矩形：逐字框跟着行的倾斜走。
    #[test]
    fn aligned_char_boxes_keep_the_tilt_of_the_detection_quad() {
        let img = RecImage::from_bgr_u8(100, 20, vec![0; 100 * 20 * 3]).expect("valid image");
        let det = [[[0.0, 0.0], [100.0, 10.0], [100.0, 30.0], [0.0, 20.0]]];
        let line = LineResult {
            text: "AB".to_string(),
            score: 0.9,
            word_info: Some(word_info(
                &["A", "B"],
                &[1, 2],
                4.0,
                vec![vec![[0, 1], [2, 4]]],
                AlignStatus::Aligned,
            )),
        };

        let out = compute_word_boxes(&[img], &det, &[line], true).expect("tilted boxes");
        for (index, box_) in out[0].iter().enumerate() {
            let width = right_x(&box_.bbox) - left_x(&box_.bbox);
            let tilt = box_.bbox[1][1] - box_.bbox[0][1];
            // 行顶边 100 px 走 10 px 下降：每个字框的下降量必须跟着自己的宽度按比例走，
            // 被替成轴对齐外接矩形时它恒为 0。
            let ratio = tilt / width;
            assert!(
                (ratio - 0.1).abs() < 0.02,
                "第 {index} 个字框没有跟着行倾斜（宽 {width}，顶边下降 {tilt}，比例 {ratio}）",
            );
        }
        assert!(
            left_x(&out[0][1].bbox) > left_x(&out[0][0].bbox),
            "倾斜行上的逐字框丢了阅读序"
        );
    }
}
