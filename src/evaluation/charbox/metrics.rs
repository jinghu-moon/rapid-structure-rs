//! 字符定位度量（docs/13 字符定位基准）。
//!
//! # 为什么所有误差都在"行局部坐标系"里算
//!
//! 基准里有 ±15° 的倾斜行和竖排行。直接在原图的 x/y 上比较左右边界会让一次旋转把
//! "左边界误差"变成"上下边界误差"的混合物，读出来的数字与算法无关、与角度有关。
//! 因此先把 GT 行四边形当作坐标轴（`u` 沿行顶边、`v` 沿行左边），再把每个框投影到
//! `(along, cross)`：**沿进给方向**的前后边界差才是"左右边界误差"（竖排时它是上下），
//! **跨轴**的差是行高对齐误差。倾斜只改变投影，不改变语义。
//!
//! # 两个 IoU
//!
//! * `iou_tight`：与 GT 字符墨迹框直接求交并。字符的墨迹高度天然不齐（`i` 与 `W`），
//!   这个口径会把"墨迹形状不同"也算成定位错误。
//! * `iou_band`：把 GT 框在**跨轴**上扩展到整行墨迹带再求交并。它只惩罚沿轴错位，
//!   也就是只惩罚"选错字符"这一件真正影响拖拽选中的事。
//!
//! 两个都要报：只看 `iou_tight` 会低估，只看 `iou_band` 会漏掉框高不对的问题。

use serde::Serialize;

use crate::evaluation::ocr::polygon_iou;

/// 四边形顶点顺序固定为 `as_quad` 产出的 左上→右上→右下→左下（旋转后仍是这个环绕方向）。
pub type Quad = [[f32; 2]; 4];

/// 投影到行局部坐标系后的区间。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    /// 沿进给方向（横排=x，竖排=y）。
    pub along: (f32, f32),
    /// 跨进给方向。
    pub cross: (f32, f32),
}

/// 一行文本的局部坐标系。
#[derive(Debug, Clone, Copy)]
pub struct LineFrame {
    origin: [f32; 2],
    u: [f32; 2],
    v: [f32; 2],
    /// 进给方向是不是 `u`。竖排时进给沿 `v`。
    advance_is_u: bool,
}

impl LineFrame {
    /// 由行四边形建系；退化（边长 < 1e-3）时返回 `None`，调用方必须把它当成
    /// "这一行无法度量"，而不是当成零误差。
    pub fn from_quad(quad: &Quad, layout: &str) -> Option<Self> {
        let origin = quad[0];
        let u = normalize(sub(quad[1], origin))?;
        let v = normalize(sub(quad[3], origin))?;
        Some(Self {
            origin,
            u,
            v,
            advance_is_u: layout != "vertical",
        })
    }

    fn axis_of_along(&self) -> [f32; 2] {
        if self.advance_is_u { self.u } else { self.v }
    }

    fn axis_of_cross(&self) -> [f32; 2] {
        if self.advance_is_u { self.v } else { self.u }
    }

    /// 把原图点投影成 `(along, cross)`。
    pub fn project_point(&self, point: [f32; 2]) -> (f32, f32) {
        let delta = sub(point, self.origin);
        (
            dot(delta, self.axis_of_along()),
            dot(delta, self.axis_of_cross()),
        )
    }

    /// 把一个四边形投影成沿轴/跨轴区间。
    pub fn project(&self, quad: &Quad) -> Extent {
        let points = quad.map(|p| self.project_point(p));
        let along: Vec<f32> = points.iter().map(|p| p.0).collect();
        let cross: Vec<f32> = points.iter().map(|p| p.1).collect();
        Extent {
            along: (
                along.iter().copied().fold(f32::INFINITY, f32::min),
                along.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            ),
            cross: (
                cross.iter().copied().fold(f32::INFINITY, f32::min),
                cross.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            ),
        }
    }

    /// 由 `(along, cross)` 区间还原回原图四边形。
    pub fn unproject(&self, along: (f32, f32), cross: (f32, f32)) -> Quad {
        let point = |a: f32, c: f32| -> [f32; 2] {
            let along_axis = self.axis_of_along();
            let cross_axis = self.axis_of_cross();
            [
                self.origin[0] + along_axis[0] * a + cross_axis[0] * c,
                self.origin[1] + along_axis[1] * a + cross_axis[1] * c,
            ]
        };
        [
            point(along.0, cross.0),
            point(along.1, cross.0),
            point(along.1, cross.1),
            point(along.0, cross.1),
        ]
    }
}

/// 一对（GT 字符框，预测字符框）的全部几何度量，单位是原图像素。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CharGeometry {
    pub iou_tight: f32,
    pub iou_band: f32,
    /// 起始边界误差：预测 - GT，沿进给方向。符号保留，系统性偏移才看得见。
    pub d_start: f32,
    pub d_end: f32,
    pub d_center: f32,
    /// 跨轴上下边界误差（预测 - GT）。
    pub d_cross_start: f32,
    pub d_cross_end: f32,
    /// 预测沿轴宽度 / GT 沿轴宽度。等宽假设的错误会以整体偏移出现，这里能单独看见。
    pub width_ratio: f32,
    /// GT 沿轴区间被预测覆盖的比例。
    pub coverage: f32,
}

/// 算一对字符框。`line_cross` 是整行墨迹带的跨轴区间，用来构造 `iou_band`。
pub fn char_geometry(
    gt: &Quad,
    predicted: &Quad,
    frame: &LineFrame,
    line_cross: (f32, f32),
) -> CharGeometry {
    let gt_extent = frame.project(gt);
    let pred_extent = frame.project(predicted);
    let band_gt = frame.unproject(gt_extent.along, line_cross);

    let overlap = (gt_extent.along.1.min(pred_extent.along.1)
        - gt_extent.along.0.max(pred_extent.along.0))
    .max(0.0);
    let gt_width = (gt_extent.along.1 - gt_extent.along.0).max(0.0);

    CharGeometry {
        iou_tight: polygon_iou(*gt, *predicted),
        iou_band: polygon_iou(band_gt, *predicted),
        d_start: pred_extent.along.0 - gt_extent.along.0,
        d_end: pred_extent.along.1 - gt_extent.along.1,
        d_center: center(&pred_extent.along) - center(&gt_extent.along),
        d_cross_start: pred_extent.cross.0 - gt_extent.cross.0,
        d_cross_end: pred_extent.cross.1 - gt_extent.cross.1,
        width_ratio: if gt_width > 0.0 {
            (pred_extent.along.1 - pred_extent.along.0) / gt_width
        } else {
            0.0
        },
        coverage: if gt_width > 0.0 {
            (overlap / gt_width).min(1.0)
        } else {
            0.0
        },
    }
}

/// 相邻两个**预测**框在进给方向上的重叠像素数（>0 表示两字符的框互相压住，
/// 一次落在重叠区的拖拽会同时选中两个字符）。
pub fn neighbour_overlap(first: &Extent, second: &Extent) -> f32 {
    (first.along.1.min(second.along.1) - first.along.0.max(second.along.0)).max(0.0)
}

/// 贪心一对一匹配：按 IoU 从高到低取，超过阈值的才成立。
///
/// 与 `evaluation::ocr::match_boxes` 同一口径，因此检测层与字符层的匹配语义一致。
pub fn match_quads(gt: &[Quad], predicted: &[Quad], threshold: f32) -> Vec<(usize, usize, f32)> {
    let mut candidates = Vec::new();
    for (gt_index, gt_quad) in gt.iter().enumerate() {
        for (pred_index, pred_quad) in predicted.iter().enumerate() {
            let iou = polygon_iou(*gt_quad, *pred_quad);
            if iou >= threshold {
                candidates.push((gt_index, pred_index, iou));
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            // IoU 相同的时候必须有一个确定的先后，否则同一份输入可以产出两种
            // 匹配集，基线就不可复现了。
            .then(a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
    });
    let mut used_gt: Vec<usize> = Vec::new();
    let mut used_pred: Vec<usize> = Vec::new();
    candidates
        .into_iter()
        .filter(|(gt_index, pred_index, _)| {
            if used_gt.contains(gt_index) || used_pred.contains(pred_index) {
                false
            } else {
                used_gt.push(*gt_index);
                used_pred.push(*pred_index);
                true
            }
        })
        .collect()
}

/// 一次"拖拽选择"的判定结果。
#[derive(Debug, Clone, Copy, Default)]
pub struct SelectionProbe {
    /// GT 落在选区里的字符数。
    pub gt_in_selection: usize,
    /// 预测落在选区里的字符数。
    pub predicted_in_selection: usize,
    /// 两者都对上的数量。
    pub true_positive: usize,
    /// 选区里有 GT 字符、但对应预测没被选中（漏选）。
    pub false_negative: usize,
    /// 选区里的预测对不上选区里的 GT 字符（多选/错选）。
    pub false_positive: usize,
}

impl SelectionProbe {
    pub fn accumulate(&mut self, other: &Self) {
        self.gt_in_selection += other.gt_in_selection;
        self.predicted_in_selection += other.predicted_in_selection;
        self.true_positive += other.true_positive;
        self.false_negative += other.false_negative;
        self.false_positive += other.false_positive;
    }
}

/// 一次选区（进给方向上的一个区间）在一行字符上的 precision/recall 计数。
///
/// `aligned[pred_index] = Option<gt_index>` 是本行内预测与 GT 的逐字对应关系，
/// 由文本闸门保证（见 `run.rs`）；没有对应关系的预测一律算错选。
pub fn selection_counts(
    frame: &LineFrame,
    gt: &[Quad],
    predicted: &[Quad],
    aligned: &[Option<usize>],
    along_range: (f32, f32),
) -> SelectionProbe {
    let inside = |quad: &Quad| {
        let middle = center(&frame.project(quad).along);
        middle >= along_range.0 && middle <= along_range.1
    };
    let gt_inside: Vec<bool> = gt.iter().map(inside).collect();
    let predicted_inside: Vec<bool> = predicted.iter().map(inside).collect();
    let mut probe = SelectionProbe {
        gt_in_selection: gt_inside.iter().filter(|inside| **inside).count(),
        predicted_in_selection: predicted_inside.iter().filter(|inside| **inside).count(),
        ..SelectionProbe::default()
    };
    for (pred_index, is_inside) in predicted_inside.iter().enumerate() {
        if !is_inside {
            continue;
        }
        let matched = aligned
            .get(pred_index)
            .copied()
            .flatten()
            .filter(|gt_index| gt_inside.get(*gt_index).copied().unwrap_or(false));
        match matched {
            Some(_) => probe.true_positive += 1,
            None => probe.false_positive += 1,
        }
    }
    for (gt_index, is_inside) in gt_inside.iter().enumerate() {
        if !is_inside {
            continue;
        }
        let covered = aligned.iter().enumerate().any(|(pred_index, slot)| {
            slot.is_some_and(|matched| matched == gt_index) && predicted_inside[pred_index]
        });
        if !covered {
            probe.false_negative += 1;
        }
    }
    probe
}

fn center(range: &(f32, f32)) -> f32 {
    (range.0 + range.1) / 2.0
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn normalize(v: [f32; 2]) -> Option<[f32; 2]> {
    let length = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if length < 1e-3 {
        return None;
    }
    Some([v[0] / length, v[1] / length])
}

#[cfg(test)]
mod tests {
    use super::{LineFrame, char_geometry, match_quads, neighbour_overlap, selection_counts};

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> [[f32; 2]; 4] {
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn an_axis_aligned_frame_measures_the_left_and_right_edges_directly() {
        let frame = LineFrame::from_quad(&rect(100.0, 40.0, 900.0, 70.0), "horizontal")
            .expect("a real rectangle has a frame");
        let gt = rect(120.0, 42.0, 150.0, 68.0);
        let predicted = rect(124.0, 42.0, 154.0, 68.0);
        let metrics = char_geometry(&gt, &predicted, &frame, (40.0, 70.0));
        assert_eq!(metrics.d_start, 4.0);
        assert_eq!(metrics.d_end, 4.0);
        assert_eq!(metrics.d_center, 4.0);
        assert_eq!(metrics.d_cross_start, 0.0);
        assert!((metrics.width_ratio - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_shifted_line_frame_reports_the_same_error_as_the_unrotated_one() {
        // 同一行、同一个偏移，只是整行旋转了 30°：沿轴误差必须不变，
        // 否则倾斜页的数字就不能和水平页比。
        let theta = 30f32.to_radians();
        let (cos, sin) = (theta.cos(), theta.sin());
        let rotate =
            |quad: &[[f32; 2]; 4]| quad.map(|[x, y]| [x * cos - y * sin, x * sin + y * cos]);
        let straight = LineFrame::from_quad(&rect(100.0, 40.0, 900.0, 70.0), "horizontal").unwrap();
        let tilted =
            LineFrame::from_quad(&rotate(&rect(100.0, 40.0, 900.0, 70.0)), "horizontal").unwrap();
        let gt = rect(120.0, 42.0, 150.0, 68.0);
        let predicted = rect(124.0, 42.0, 154.0, 68.0);
        let straight_metrics = char_geometry(&gt, &predicted, &straight, (40.0, 70.0));
        let tilted_metrics =
            char_geometry(&rotate(&gt), &rotate(&predicted), &tilted, (40.0, 70.0));
        for (a, b, label) in [
            (straight_metrics.d_start, tilted_metrics.d_start, "d_start"),
            (straight_metrics.d_end, tilted_metrics.d_end, "d_end"),
            (
                straight_metrics.d_center,
                tilted_metrics.d_center,
                "d_center",
            ),
        ] {
            assert!(
                (a - b).abs() < 1e-3,
                "{label} changed under rotation: {a} vs {b}"
            );
        }
        assert!(
            (straight_metrics.iou_tight - tilted_metrics.iou_tight).abs() < 1e-3,
            "IoU must not depend on the page rotation"
        );
    }

    #[test]
    fn a_vertical_frame_uses_the_down_axis_as_the_advance_direction() {
        let frame = LineFrame::from_quad(&rect(40.0, 100.0, 70.0, 900.0), "vertical").unwrap();
        let gt = rect(42.0, 120.0, 68.0, 150.0);
        let predicted = rect(42.0, 126.0, 68.0, 156.0);
        let metrics = char_geometry(&gt, &predicted, &frame, (40.0, 70.0));
        // 竖排的"前后边界"是上下，所以沿轴误差来自 y；x 上的差别算跨轴。
        assert_eq!(metrics.d_start, 6.0);
        assert_eq!(metrics.d_end, 6.0);
        assert_eq!(metrics.d_cross_start, 0.0);
    }

    #[test]
    fn the_band_iou_ignores_cross_axis_shape_and_keeps_along_axis_error() {
        let frame = LineFrame::from_quad(&rect(0.0, 0.0, 400.0, 30.0), "horizontal").unwrap();
        // GT 墨迹只有 10px 高，预测框占满整行带：沿轴完全对齐。
        let gt = rect(10.0, 12.0, 30.0, 22.0);
        let predicted = rect(10.0, 0.0, 30.0, 30.0);
        let metrics = char_geometry(&gt, &predicted, &frame, (0.0, 30.0));
        assert!(
            (metrics.iou_tight - 1.0 / 3.0).abs() < 1e-4,
            "tight IoU {}",
            metrics.iou_tight
        );
        assert!(
            (metrics.iou_band - 1.0).abs() < 1e-6,
            "band IoU {}",
            metrics.iou_band
        );
        assert_eq!(metrics.d_center, 0.0);
    }

    #[test]
    fn a_degenerate_line_quad_cannot_be_measured() {
        assert!(LineFrame::from_quad(&rect(10.0, 10.0, 10.0, 40.0), "horizontal").is_none());
    }

    #[test]
    fn matching_prefers_the_best_iou_and_never_reuses_a_box() {
        let gt = [rect(0.0, 0.0, 20.0, 20.0), rect(30.0, 0.0, 50.0, 20.0)];
        let predicted = [rect(1.0, 0.0, 21.0, 20.0), rect(0.0, 0.0, 40.0, 20.0)];
        let pairs = match_quads(&gt, &predicted, 0.1);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, 0);
        assert_eq!(pairs[0].1, 0);
        assert_eq!(pairs[1].0, 1);
        assert_eq!(pairs[1].1, 1);
        assert!(
            pairs
                .iter()
                .map(|p| p.0)
                .collect::<Vec<_>>()
                .windows(2)
                .all(|w| w[0] < w[1])
        );
    }

    #[test]
    fn matching_drops_everything_below_the_threshold() {
        let pairs = match_quads(
            &[rect(0.0, 0.0, 10.0, 10.0)],
            &[rect(90.0, 90.0, 100.0, 100.0)],
            0.3,
        );
        assert!(pairs.is_empty());
    }

    #[test]
    fn neighbour_overlap_is_positive_only_when_the_boxes_share_ground() {
        let frame = LineFrame::from_quad(&rect(0.0, 0.0, 400.0, 30.0), "horizontal").unwrap();
        let first = frame.project(&rect(10.0, 0.0, 40.0, 30.0));
        let second = frame.project(&rect(35.0, 0.0, 60.0, 30.0));
        assert_eq!(neighbour_overlap(&first, &second), 5.0);
        let apart = frame.project(&rect(50.0, 0.0, 60.0, 30.0));
        assert_eq!(neighbour_overlap(&first, &apart), 0.0);
    }

    #[test]
    fn a_selection_probe_counts_a_missed_character_and_a_stray_prediction() {
        let frame = LineFrame::from_quad(&rect(0.0, 0.0, 400.0, 30.0), "horizontal").unwrap();
        let gt = [rect(10.0, 0.0, 30.0, 30.0), rect(40.0, 0.0, 60.0, 30.0)];
        let predicted = [rect(10.0, 0.0, 30.0, 30.0), rect(40.0, 0.0, 60.0, 30.0)];
        let aligned = [Some(0usize), Some(1usize)];
        // 选区只盖住第二个 GT 字符；第一个预测框中心在 20，落在区外。
        let probe = selection_counts(&frame, &gt, &predicted, &aligned, (35.0, 65.0));
        assert_eq!(probe.gt_in_selection, 1);
        assert_eq!(probe.predicted_in_selection, 1);
        assert_eq!(probe.true_positive, 1);
        assert_eq!(probe.false_positive, 0);
        assert_eq!(probe.false_negative, 0);
        // 把第二个预测框挪走：GT 字符仍在选区里，于是这是一次漏选。
        let moved = [rect(10.0, 0.0, 30.0, 30.0), rect(300.0, 0.0, 320.0, 30.0)];
        let probe = selection_counts(&frame, &gt, &moved, &aligned, (35.0, 65.0));
        assert_eq!(probe.true_positive, 0);
        assert_eq!(probe.false_negative, 1);
        assert_eq!(probe.predicted_in_selection, 0);
    }

    #[test]
    fn an_unmatched_prediction_inside_the_selection_is_a_false_positive() {
        let frame = LineFrame::from_quad(&rect(0.0, 0.0, 400.0, 30.0), "horizontal").unwrap();
        let gt = [rect(10.0, 0.0, 30.0, 30.0)];
        let predicted = [rect(12.0, 0.0, 32.0, 30.0), rect(15.0, 0.0, 35.0, 30.0)];
        let aligned = [Some(0usize), None];
        let probe = selection_counts(&frame, &gt, &predicted, &aligned, (5.0, 40.0));
        assert_eq!(probe.true_positive, 1);
        assert_eq!(probe.false_positive, 1);
        assert_eq!(probe.false_negative, 0);
    }
}
