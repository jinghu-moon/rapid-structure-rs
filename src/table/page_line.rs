//! 页文本行的规范表示（`docs/12` §4.2 P1-4，D-2 / 评审二.7）。
//!
//! 同一个页面文本区域在表格管线的两条下游路径上各需要一种形状：
//! - 定位器 [`crate::table::detection::PageLine`] 需要**四边形**；
//! - 回填 [`crate::table::cell_content_backfill::TextLine`] 需要**裁剪后的轴对齐 bbox**。
//!
//! 过去这两份是在装配点（`ocr::pipeline::rapid_structure::table_source_lines`）里用一个循环
//! 各自拼出来的 —— 同处构造能防分叉，但"bbox 的裁剪规则"仍散落在调用点。本模块把它收敛成
//! **一次装配、多处只读**的规范视图：`PageTextLine::new` 是 bbox 的唯一生成点，两个旧视图
//! 由它显式导出。行为等价、非破坏性：旧结构 `PageLine` / `TextLine` 原样保留、消费者不变，
//! 只是它们的数据来源收敛到同一个 `PageTextLine`。
//!
//! 刻意**不**放进 `ocr::types`：它服务的是页面级装配，而非 OCR 会话内部类型。也刻意**不**读
//! [`crate::api::OcrRegion`]：把区域过滤 / 取字段留在装配点，`table` 域就无需反向依赖 `api`
//! 的输出类型（见 `src/table/mod.rs` 的分域约束）；`PageTextLine::new` 只吃原始几何 + 文本 + 分数。

use crate::Quad;
use crate::table::cell_content_backfill::TextLine;
use crate::table::detection::PageLine;

/// 页级文本行的规范视图：一次装配、多处只读。
#[derive(Debug, Clone, PartialEq)]
pub struct PageTextLine {
    /// 该行的 `index` 语义由装配点决定（当前是区域在 `OcrOutput::regions` 中的下标）。
    pub index: usize,
    /// 识别文本。
    pub text: String,
    /// det 文本框，四个角点，原图像素坐标。
    pub polygon: Quad,
    /// 由 `polygon` 按既有裁剪规则导出的轴对齐框 `[x_min, y_min, x_max, y_max]`。
    pub bbox: [f32; 4],
    /// 识别置信度。
    pub score: f32,
}

impl PageTextLine {
    /// 由几何 + 文本 + 分数装配一行；`bbox` 在这里**唯一一次**从 `polygon` 导出，
    /// 用的是与合并前逐位一致的 `aabb_from_flat8`（即 `axis_aligned_bbox` 委托到的实现）。
    pub fn new(index: usize, polygon: Quad, text: String, score: f32) -> Self {
        let flat = [
            polygon[0][0],
            polygon[0][1],
            polygon[1][0],
            polygon[1][1],
            polygon[2][0],
            polygon[2][1],
            polygon[3][0],
            polygon[3][1],
        ];
        Self {
            index,
            text,
            polygon,
            bbox: crate::vision::geometry::aabb_from_flat8(&flat),
            score,
        }
    }

    /// 导出定位器视图（四边形 + 文本 + 分数）。
    pub fn to_page_line(&self) -> PageLine {
        PageLine {
            polygon: self.polygon,
            text: self.text.clone(),
            score: self.score,
        }
    }

    /// 导出回填视图（轴对齐 bbox + 文本 + 分数）。
    pub fn to_text_line(&self) -> TextLine {
        TextLine {
            bbox: self.bbox,
            text: self.text.clone(),
            score: self.score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PageTextLine;
    use crate::table::cell_content_backfill::axis_aligned_bbox;

    /// 规范视图导出的两个旧视图，必须与合并前装配点手拼的结果逐字段一致。
    #[test]
    fn derived_views_match_the_pre_merge_construction() {
        // 一个倾斜的四边形：分量极值不等于任一角点，能同时压住 bbox 的两条实现。
        let polygon: crate::Quad = [[3.0, 8.0], [10.0, 2.0], [12.0, 6.0], [5.0, 11.0]];
        let flat = [
            polygon[0][0],
            polygon[0][1],
            polygon[1][0],
            polygon[1][1],
            polygon[2][0],
            polygon[2][1],
            polygon[3][0],
            polygon[3][1],
        ];

        let line = PageTextLine::new(7, polygon, "行文本".into(), 0.87);

        // bbox 与装配点原来调用的 axis_aligned_bbox(&flat) 逐位相同。
        assert_eq!(line.bbox, axis_aligned_bbox(&flat));
        assert_eq!(line.bbox, [3.0, 2.0, 12.0, 11.0]);

        let page = line.to_page_line();
        assert_eq!(page.polygon, polygon);
        assert_eq!(page.text, "行文本");
        assert_eq!(page.score, 0.87);

        let text = line.to_text_line();
        assert_eq!(text.bbox, line.bbox);
        assert_eq!(text.text, "行文本");
        assert_eq!(text.score, 0.87);

        assert_eq!(line.index, 7);
    }

    /// 边界裁剪：负坐标与单点退化仍走同一条 `aabb_from_flat8`，与旧实现一致。
    #[test]
    fn degenerate_and_negative_polygon_bbox_matches_reference() {
        let polygon: crate::Quad = [[-4.0, -1.0], [-4.0, -1.0], [0.0, 5.0], [0.0, 5.0]];
        let flat = [
            polygon[0][0],
            polygon[0][1],
            polygon[1][0],
            polygon[1][1],
            polygon[2][0],
            polygon[2][1],
            polygon[3][0],
            polygon[3][1],
        ];
        let line = PageTextLine::new(0, polygon, String::new(), 0.0);
        assert_eq!(line.bbox, axis_aligned_bbox(&flat));
        assert_eq!(line.bbox, [-4.0, -1.0, 0.0, 5.0]);
    }
}
