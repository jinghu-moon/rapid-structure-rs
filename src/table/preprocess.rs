//! 表格结构模型的输入预处理：**纯函数、零外部资产**。
//!
//! 语义逐行对应参考实现 `rapid_table/table_structure/pp_structure/pre_process.py`
//! 的 `TablePreprocess.__call__`（60 行），顺序不可交换：
//!
//! ```text
//! resize_image  →  normalize  →  pad_img  →  to_chw
//! ```
//!
//! # 三个容易被"顺手改好"从而改错的点
//!
//! 1. **先归一化、后补零**（`normalize` 在 `pad_img` 之前）。补零发生在归一化**之后**，
//!    所以画布右侧与下侧的填充值是 `0.0`，而不是"黑色像素归一化后的值"
//!    （后者是 `(0/255 - mean) / std`，即 `(-2.12, -2.04, -1.80)`）。两者在数值上相差
//!    两个数量级，改错不会报错，只会让所有预测悄悄偏移。
//! 2. **通道顺序是 BGR**：参考实现用 `cv2.imread` 读图（BGR）后直接做
//!    `(img * scale - mean) / std`，**没有做 RGB 转换**，因此 `mean[0] = 0.485` 施加在
//!    **蓝**通道上。这不是笔误 —— PaddleOCR 的表格结构预处理 ops 里 `DecodeImage` 用的是
//!    `img_mode: BGR`，参考实现忠实复刻了它。按 RGB 顺序施加会得到一个"看起来合理"的
//!    输入，但模型是在 BGR 上训练的。
//! 3. **补零是左上对齐**（`padding_img[:h, :w, :] = img`），不是居中。这与
//!    [`crate::table::decode::TableShape`] 的 `shape_list` 前两项（`h, w`）配套：坐标
//!    反归一化假设的是"原图占画布左上角"。
//!
//! # 与参考实现的唯一刻意差异
//!
//! 参考实现的 `int(h * ratio)` 对退化输入（`resize_h == 0` 或 `resize_w == 0`）会直接把
//! 零尺寸交给 `cv2.resize`，在那里抛一个与表格无关的异常。这里改成显式的
//! [`RapidOcrError::InvalidImage`]：零尺寸输入**不可能是**一张表格，静默裁剪出一个
//! 单像素画布才是更坏的结果。

use ndarray::Array4;

use crate::{
    config::RecImage,
    error::{RapidOcrError, Result},
    vision::image_backend::resize_image,
};

/// 归一化均值，按 **BGR** 通道顺序（见模块文档第 2 点）。
pub const TABLE_STRUCTURE_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
/// 归一化标准差，按 **BGR** 通道顺序。
pub const TABLE_STRUCTURE_STD: [f32; 3] = [0.229, 0.224, 0.225];
/// 归一化缩放系数（`1 / 255`）。
pub const TABLE_STRUCTURE_SCALE: f32 = 1.0 / 255.0;

/// 一次预处理留下的几何信息。
///
/// `original_*` 与 `resize_*` 是坐标反归一化（[`crate::table::decode::TableShape`]）与
/// 几何匹配（回填阶段）都要用的量，因此与张量一起返回，而不是让调用方自己重算一遍 ——
/// 重算一遍意味着两处 `int()` 截断规则要永远保持一致。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableCanvas {
    /// 画布边长（= 输入张量的 H = W）。
    pub side_len: usize,
    /// 原图尺寸。
    pub original_height: usize,
    pub original_width: usize,
    /// 缩放后的尺寸；`resize_height * resize_width` 是画布左上角被真实像素占据的区域。
    pub resize_height: usize,
    pub resize_width: usize,
    /// 参考实现的 `ratio = max_len / max(h, w)`。
    pub ratio: f32,
}

impl TableCanvas {
    /// 参考实现的 `shape_list` 前两项（`[h, w, ratio, ratio, 488, 488]` 里的 `h` / `w`）。
    pub fn original_dimensions(&self) -> (f32, f32) {
        (self.original_height as f32, self.original_width as f32)
    }

    /// 画布上被真实像素占据的区域，`[x0, y0, x1, y1]`（左上对齐）。
    pub fn content_bbox(&self) -> [f32; 4] {
        [
            0.0,
            0.0,
            self.resize_width as f32,
            self.resize_height as f32,
        ]
    }
}

/// 参考实现的 `resize_image`：`ratio = side_len / max(h, w)`，再对 `h` / `w` 分别取
/// `int(...)`（**截断**，不是四舍五入）。
///
/// 截断而非舍入不是细节：`488/488 * 488` 在 f64 下可能得到 `487.999…`，两种规则会给出
/// 相差一个像素的尺寸，而画布尺寸决定了后面所有坐标的比例。
pub fn resize_dimensions(
    height: usize,
    width: usize,
    side_len: usize,
) -> Result<(usize, usize, f32)> {
    if height == 0 || width == 0 {
        return Err(RapidOcrError::InvalidImage(
            "table structure preprocessing got an image with a zero side".to_string(),
        ));
    }
    if side_len == 0 {
        return Err(RapidOcrError::Config(
            "table structure canvas side length must be greater than zero".to_string(),
        ));
    }
    let longest = height.max(width) as f64;
    let ratio = side_len as f64 / longest;
    let resize_height = (height as f64 * ratio) as usize;
    let resize_width = (width as f64 * ratio) as usize;
    if resize_height == 0 || resize_width == 0 {
        return Err(RapidOcrError::InvalidImage(format!(
            "table structure resize collapsed to {resize_width}x{resize_height} for a \
             {width}x{height} image on a {side_len} canvas"
        )));
    }
    if resize_height > side_len || resize_width > side_len {
        return Err(RapidOcrError::InvalidImage(format!(
            "table structure resize produced {resize_width}x{resize_height}, which does not fit \
             the {side_len}x{side_len} canvas"
        )));
    }
    Ok((resize_height, resize_width, ratio as f32))
}

/// 预处理一张表图，产出 `[1, 3, side_len, side_len]` 的 `f32` 张量。
///
/// 输入按 **BGR** 解释（见模块文档）。
pub fn table_structure_input(
    image: &RecImage,
    side_len: usize,
) -> Result<(Array4<f32>, TableCanvas)> {
    let (original_height, original_width) = (image.height(), image.width());
    let (resize_height, resize_width, ratio) =
        resize_dimensions(original_height, original_width, side_len)?;

    let resized = resize_image(image, resize_width, resize_height)?;
    let resized_owned = resized.as_bgr_cow();
    let resized = resized_owned.as_ref();
    debug_assert_eq!(resized.len(), resize_height * resize_width * 3);

    // 画布先按"归一化之后补零"填充：零是补零值本身，不是黑色像素的归一化结果。
    let mut tensor = Array4::<f32>::zeros((1, 3, side_len, side_len));
    for channel in 0..3usize {
        let mean = TABLE_STRUCTURE_MEAN[channel];
        let std = TABLE_STRUCTURE_STD[channel];
        let mut plane = tensor.slice_mut(ndarray::s![0, channel, .., ..]);
        for y in 0..resize_height {
            for x in 0..resize_width {
                let raw = resized[(y * resize_width + x) * 3 + channel] as f32;
                plane[[y, x]] = (raw * TABLE_STRUCTURE_SCALE - mean) / std;
            }
        }
    }

    let canvas = TableCanvas {
        side_len,
        original_height,
        original_width,
        resize_height,
        resize_width,
        ratio,
    };
    Ok((tensor, canvas))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, bgr: [u8; 3]) -> RecImage {
        let mut data = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            data.extend_from_slice(&bgr);
        }
        RecImage::from_bgr_u8(width, height, data).expect("synthetic image must be valid")
    }

    #[test]
    fn resize_uses_truncation_like_the_reference() {
        // 参考实现 `int(h * ratio)`：高比宽长的图，ratio = 488/h，h 侧应恰好落在 488。
        let (h, w, ratio) = resize_dimensions(1000, 500, 488).expect("valid");
        assert_eq!(h, 488);
        assert_eq!(w, 244);
        assert!((ratio - 0.488).abs() < 1e-6);
    }

    #[test]
    fn a_square_image_fills_the_canvas() {
        let (h, w, _) = resize_dimensions(488, 488, 488).expect("valid");
        assert_eq!((h, w), (488, 488));
    }

    #[test]
    fn a_degenerate_resize_is_rejected_rather_than_silently_cropped() {
        // 参考实现会把零尺寸直接交给 cv2.resize；这里必须显式报错。
        let error = resize_dimensions(1, 100_000, 488).expect_err("must reject");
        assert!(
            matches!(error, RapidOcrError::InvalidImage(_)),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_zero_sided_image_is_rejected() {
        assert!(resize_dimensions(0, 10, 488).is_err());
        assert!(resize_dimensions(10, 0, 488).is_err());
    }

    #[test]
    fn real_pixels_are_normalized_and_padding_stays_exactly_zero() {
        // 1 高 × 2 宽放到 4×4：ratio = 4/2 = 2 → resize 2×4，**下两行**是补零区。
        // 真实像素按 `(v/255 - mean) / std`，补零区必须是精确的 0.0。
        let image = solid(2, 1, [255, 255, 255]);
        let (tensor, canvas) = table_structure_input(&image, 4).expect("valid");
        assert_eq!((canvas.resize_height, canvas.resize_width), (2, 4));
        for channel in 0..3 {
            let expected = (255.0 * TABLE_STRUCTURE_SCALE - TABLE_STRUCTURE_MEAN[channel])
                / TABLE_STRUCTURE_STD[channel];
            for y in 0..2 {
                for x in 0..4 {
                    let value = tensor[[0, channel, y, x]];
                    assert!(
                        (value - expected).abs() < 1e-5,
                        "channel {channel} at ({y},{x}) is {value}, expected {expected}"
                    );
                }
            }
            for y in 2..4 {
                for x in 0..4 {
                    assert_eq!(
                        tensor[[0, channel, y, x]],
                        0.0,
                        "channel {channel} at ({y},{x}) must be the padding zero"
                    );
                }
            }
        }
    }

    #[test]
    fn the_canvas_is_left_top_aligned_so_the_right_and_bottom_are_exactly_zero() {
        // 2x1 的图（高 2、宽 1）放到 4x4：ratio = 4/2 = 2 → resize 4x2，右侧两列补零。
        let image = solid(1, 2, [10, 20, 30]);
        let (tensor, canvas) = table_structure_input(&image, 4).expect("valid");
        assert_eq!((canvas.resize_height, canvas.resize_width), (4, 2));
        for channel in 0..3 {
            for y in 0..4 {
                for x in 2..4 {
                    assert_eq!(
                        tensor[[0, channel, y, x]],
                        0.0,
                        "channel {channel} at ({y},{x}) must be the padding zero"
                    );
                }
            }
        }
    }

    #[test]
    fn channels_are_normalized_in_bgr_order() {
        // 纯蓝 (255, 0, 0)：通道 0 应命中 mean[0]=0.485 / std[0]=0.229。
        let image = solid(4, 4, [255, 0, 0]);
        let (tensor, _) = table_structure_input(&image, 4).expect("valid");
        let expected_blue = (255.0 * TABLE_STRUCTURE_SCALE - 0.485) / 0.229;
        let expected_black_g = (0.0 - 0.456) / 0.224;
        assert!((tensor[[0, 0, 0, 0]] - expected_blue).abs() < 1e-5);
        assert!((tensor[[0, 1, 0, 0]] - expected_black_g).abs() < 1e-5);
        // 反向对照：若有人把通道顺序改成 RGB，通道 0 会改用 mean 0.406 / std 0.225，
        // 得到 2.64 而不是 2.249 —— 两者相差 0.39，比上面的 1e-5 容差大三个数量级。
        // 只有通道 0 能区分 BGR 与 RGB（通道 1 的均值/方差在两种顺序下恰好相同），
        // 所以这条对照断言是必须的：没有它，把 BGR 改成 RGB 时上面两个断言会有一个
        // 恰好仍然通过（通道 1），测试变成"看起来还在守着通道顺序"。
        let if_it_were_rgb = (255.0 * TABLE_STRUCTURE_SCALE - 0.406) / 0.225;
        assert!(
            (tensor[[0, 0, 0, 0]] - if_it_were_rgb).abs() > 0.3,
            "channel 0 matches the RGB interpretation too, so this test cannot tell BGR from RGB"
        );
    }

    #[test]
    fn the_tensor_shape_is_one_by_three_by_side_by_side() {
        let image = solid(300, 100, [1, 2, 3]);
        let (tensor, canvas) = table_structure_input(&image, 488).expect("valid");
        assert_eq!(tensor.shape(), &[1, 3, 488, 488]);
        assert_eq!(canvas.original_dimensions(), (100.0, 300.0));
        assert_eq!(canvas.content_bbox(), [0.0, 0.0, 488.0, 162.0]);
    }
}
