//! 共享几何层（`docs/12` §4.2 P1-1）：**polygon → 轴对齐外接框、轴对齐 IoU、polygon 面积**
//! 的唯一实现。
//!
//! 刻意**不**合并的东西（语义不同，合并会引入难查的判定漂移）：
//! - `evaluation::ocr::polygon_iou` —— 凸四边形裁剪后的真实 polygon IoU；
//! - `formula::detect::iou` —— 参与类别感知 NMS，边界是 `>` 且与 `offset_box` 绑定；
//! - `formula::route::Rect::iou` —— f32、`union <= 0` 早退，与本页 f64 口径不同；
//! - `det::postprocess` 的 f32 面积 / 取整夹边到整数的 crop（`rotate_crop`、i32 轮廓）；
//! - 各域的贪心 / 抑制 / `covered_ratio` 逻辑。
//!
//! 输出统一沿用仓库既有的 `[f32; 4]` = `[x_min, y_min, x_max, y_max]` 形态，而不是新造
//! `Aabb` 类型：`axis_aligned_bbox` / `polygon_to_bbox` / `rectangle_iou` 等公开函数与
//! `CellSlot::bbox` / `TextLine::bbox` 字段全都以该数组承载，改类型会把一次"内部单点化"
//! 放大成破坏性 API 变更。这里只做**实现收敛**，被收敛的公开函数逐个委托到本模块。

use crate::Quad;

/// 一组点 → 轴对齐外接框 `[x_min, y_min, x_max, y_max]`（f32，不夹边、不取整）。
///
/// 用 `±INFINITY` 起算：对非空点集，结果与"用首元素起算"的旧实现逐位一致。
pub(crate) fn aabb_of(points: &[[f32; 2]]) -> [f32; 4] {
    let mut out = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for point in points {
        out[0] = out[0].min(point[0]);
        out[1] = out[1].min(point[1]);
        out[2] = out[2].max(point[0]);
        out[3] = out[3].max(point[1]);
    }
    out
}

/// 四边形 → 轴对齐外接框（[`aabb_of`] 的四点特化）。
pub(crate) fn aabb_from_quad(quad: &Quad) -> [f32; 4] {
    aabb_of(quad.as_slice())
}

/// 8 点多边形（`[x0, y0, x1, y1, x2, y2, x3, y3]`）→ 轴对齐外接框。
///
/// 与参考实现 `TableMatch.match_result` 的
/// `[np.min(pred_box[0::2]), np.min(pred_box[1::2]), np.max(pred_box[0::2]), np.max(pred_box[1::2])]`
/// 逐位一致；`cell_content_backfill::axis_aligned_bbox` 与 `decode::polygon_to_bbox` 都委托到这里。
pub(crate) fn aabb_from_flat8(polygon: &[f32; 8]) -> [f32; 4] {
    let mut x_min = polygon[0];
    let mut x_max = polygon[0];
    let mut y_min = polygon[1];
    let mut y_max = polygon[1];
    for pair in polygon.as_chunks::<2>().0 {
        let (x, y) = (pair[0], pair[1]);
        if x < x_min {
            x_min = x;
        }
        if x > x_max {
            x_max = x;
        }
        if y < y_min {
            y_min = y;
        }
        if y > y_max {
            y_max = y;
        }
    }
    [x_min, y_min, x_max, y_max]
}

/// 轴对齐框的 IoU（f64）。边界口径与 `cell_content_backfill::rectangle_iou` 逐位一致：
/// 任一方向重叠 `<= 0.0`（即仅共边）早退为 `0.0`，退化/零面积不产生 `NaN`。
pub(crate) fn aabb_iou(a: [f32; 4], b: [f32; 4]) -> f64 {
    let (ax0, ay0, ax1, ay1) = (a[0] as f64, a[1] as f64, a[2] as f64, a[3] as f64);
    let (bx0, by0, bx1, by1) = (b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64);

    let area_a = (ax1 - ax0) * (ay1 - ay0);
    let area_b = (bx1 - bx0) * (by1 - by0);

    let x_overlap = ax1.min(bx1) - ax0.max(bx0);
    let y_overlap = ay1.min(by1) - ay0.max(by0);
    if x_overlap <= 0.0 || y_overlap <= 0.0 {
        return 0.0;
    }

    let intersect = x_overlap * y_overlap;
    let denominator = area_a + area_b - intersect;
    if denominator <= 0.0 {
        return 0.0;
    }
    intersect / denominator
}

/// polygon 的鞋带公式面积（f64，**有符号**：顶点逆时针为正、顺时针为负）。
///
/// 与 `det::postprocess::polygon_area_f64` 逐位一致（调用方按需取 `.abs()`）；少于 3 点返回 `0.0`。
pub(crate) fn polygon_area(points: &[[f32; 2]]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }

    let mut sum = 0.0_f64;
    for i in 0..points.len() {
        let p0 = points[i];
        let p1 = points[(i + 1) % points.len()];
        sum += f64::from(p0[0]) * f64::from(p1[1]) - f64::from(p1[0]) * f64::from(p0[1]);
    }
    0.5 * sum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立参考实现（不复用被测函数），用于对拍 `aabb_of` / `aabb_from_flat8` / `aabb_iou`。
    fn ref_aabb(points: &[[f32; 2]]) -> [f32; 4] {
        let xs: Vec<f32> = points.iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = points.iter().map(|p| p[1]).collect();
        [
            xs.iter().copied().fold(f32::INFINITY, f32::min),
            ys.iter().copied().fold(f32::INFINITY, f32::min),
            xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        ]
    }

    fn ref_iou(a: [f32; 4], b: [f32; 4]) -> f64 {
        let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0) as f64;
        let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0) as f64;
        let inter = ix * iy;
        let area_a = (a[2] - a[0]).max(0.0) as f64 * (a[3] - a[1]).max(0.0) as f64;
        let area_b = (b[2] - b[0]).max(0.0) as f64 * (b[3] - b[1]).max(0.0) as f64;
        let union = area_a + area_b - inter;
        if union <= 0.0 { 0.0 } else { inter / union }
    }

    #[test]
    fn aabb_handles_degenerate_negative_and_single_point() {
        // 单点：退化零面积框。
        assert_eq!(aabb_of(&[[3.0, -2.0]]), [3.0, -2.0, 3.0, -2.0]);
        // 负坐标 + 乱序顶点。
        let pts = [[-1.0, 5.0], [4.0, -3.0], [0.0, 0.0], [2.0, 9.0]];
        assert_eq!(aabb_of(&pts), [-1.0, -3.0, 4.0, 9.0]);
        assert_eq!(aabb_of(&pts), ref_aabb(&pts));
    }

    #[test]
    fn aabb_from_quad_matches_aabb_of() {
        let quad: Quad = [[10.0, 0.0], [20.0, 0.0], [20.0, 30.0], [10.0, 30.0]];
        assert_eq!(aabb_from_quad(&quad), [10.0, 0.0, 20.0, 30.0]);
        assert_eq!(aabb_from_quad(&quad), aabb_of(quad.as_slice()));
    }

    #[test]
    fn aabb_from_flat8_agrees_with_aabb_of_on_the_same_points() {
        // 四边形以 8 点扁平形式给出（左上、右上、右下、左下）。
        let flat = [5.0_f32, 5.0, 50.0, 5.0, 50.0, 40.0, 5.0, 40.0];
        let points: Vec<[f32; 2]> = flat
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| [p[0], p[1]])
            .collect();
        assert_eq!(aabb_from_flat8(&flat), aabb_of(&points));
        assert_eq!(aabb_from_flat8(&flat), [5.0, 5.0, 50.0, 40.0]);
    }

    #[test]
    fn aabb_iou_boundaries_and_reference() {
        // 完全重合 → 1.0。
        assert!((aabb_iou([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 10.0, 10.0]) - 1.0).abs() < 1e-12);
        // 包含关系：小框完全在大框内 → inter/union = 小/大 = 0.25×0.25? 用 1/16。
        let contained = aabb_iou([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 5.0, 5.0]);
        assert!((contained - 0.25).abs() < 1e-12);
        assert!((contained - ref_iou([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 5.0, 5.0])).abs() < 1e-12);
        // 恰好共边（重叠长度 0）→ 早退 0.0（`<=` 边界，逐处保留原样）。
        assert_eq!(
            aabb_iou([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0]),
            0.0
        );
        // 完全分离 → 0.0。
        assert_eq!(
            aabb_iou([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 30.0, 30.0]),
            0.0
        );
        // 零面积框（退化）不产生 NaN，返回 0.0。
        let nan = aabb_iou([3.0, 3.0, 3.0, 3.0], [3.0, 3.0, 3.0, 3.0]);
        assert!(!nan.is_nan());
        assert_eq!(nan, 0.0);
    }

    #[test]
    fn polygon_area_is_signed_shoelace() {
        // 3×3 单位正方形，逆时针 → +9.0。
        let ccw = [[0.0_f32, 0.0], [3.0, 0.0], [3.0, 3.0], [0.0, 3.0]];
        assert!((polygon_area(&ccw) - 9.0).abs() < 1e-9);
        // 顺时针 → -9.0（有符号，调用方取 abs）。
        let cw: Vec<[f32; 2]> = ccw.iter().rev().copied().collect();
        assert!((polygon_area(&cw) + 9.0).abs() < 1e-9);
        // 少于 3 点退化 → 0。
        assert_eq!(polygon_area(&[[0.0, 0.0], [1.0, 1.0]]), 0.0);
    }
}
