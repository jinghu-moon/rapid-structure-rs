//! 共享归一化层（`docs/12` §4.2 P1-3）：rec 与 cls 那份**确定同形**的有符号单位化
//! LUT 的唯一实现。
//!
//! 刻意**不**合并的东西（语义/热路径不同，评审二.6 明确要求暂缓，逐像素对拍后另议）：
//! - `ocr::det::preprocess` 的 AVX2 归一化路径；
//! - `table::preprocess` 的 ImageNet `(raw*SCALE-mean)/std` 内联归一化；
//! - 各域不同的 resize 尺寸策略、CHW 写入顺序与 padding 语义。
//!
//! 收敛的只有 `x * (2.0 / 255.0) - 1.0`（并把 `-0.0` 归一为 `0.0`）这一份逐字节输入的
//! 查表实现：rec 与 cls 的 `signed_unit_lut` 与 `normalize_to_signed_unit` 原本逐字相同，
//! 这里收成单点，两处调用点委托过来，浮点运算顺序不变（仍是先乘后减、单次比较）。

use std::sync::OnceLock;

/// 256 项有符号单位化查表：`lut[v] == v as f32 * (2.0 / 255.0) - 1.0`，`-0.0` 归一为 `0.0`。
///
/// 与合并前 `ocr::rec::preprocess` / `ocr::cls::preprocess` 的局部实现逐元素一致。
pub(crate) fn signed_unit_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut out = [0.0_f32; 256];
        let mut i = 0usize;
        while i < 256 {
            let mut v = i as f32 * (2.0 / 255.0) - 1.0;
            if v == -0.0 {
                v = 0.0;
            }
            out[i] = v;
            i += 1;
        }
        out
    })
}

/// 单字节 → 有符号单位浮点（查表）。热路径逐像素调用，保持 `#[inline]`。
#[inline]
pub(crate) fn normalize_to_signed_unit(v: u8) -> f32 {
    signed_unit_lut()[v as usize]
}

#[cfg(test)]
mod tests {
    use super::signed_unit_lut;

    /// 合并前的两处局部实现就是这个闭式；全域逐项对拍，保证 LUT 数值与旧实现完全一致。
    fn reference_signed_unit(v: usize) -> f32 {
        let mut out = v as f32 * (2.0 / 255.0) - 1.0;
        if out == -0.0 {
            out = 0.0;
        }
        out
    }

    #[test]
    fn lut_matches_reference_over_the_full_input_range() {
        let lut = signed_unit_lut();
        assert_eq!(lut.len(), 256);
        for (i, entry) in lut.iter().enumerate() {
            // 逐位（含符号）比较：`to_bytes` 会把 `-0.0` 与 `0.0` 区分开，
            // 而闭式实现已把 `-0.0` 归一为 `0.0`，两侧字节模式必须一致。
            assert_eq!(
                entry.to_be_bytes(),
                reference_signed_unit(i).to_be_bytes(),
                "LUT entry {i} differs from the pre-merge closed form"
            );
        }
    }

    #[test]
    fn lut_endpoints_and_negative_zero_normalization() {
        let lut = signed_unit_lut();
        // 端点：0 → -1.0，255 → +1.0。
        assert_eq!(lut[0], -1.0);
        assert_eq!(lut[255], 1.0);
        // 中点 127.5 处落在 (-1, 1) 之间，且没有 `-0.0`。
        assert!(lut[127] < 0.0 && lut[128] > 0.0);
        for entry in lut.iter() {
            assert!(
                !entry.is_sign_negative() || *entry != 0.0,
                "LUT must not contain -0.0"
            );
        }
    }

    #[test]
    fn lut_is_a_stable_singleton_across_calls() {
        // OnceLock 保证多处调用共享同一块静态内存（rec/cls 委托后行为一致的前提）。
        let a = signed_unit_lut() as *const [f32; 256];
        let b = signed_unit_lut() as *const [f32; 256];
        assert!(std::ptr::eq(a, b));
    }
}
