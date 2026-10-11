//! 字节数组 SHA-256 小写十六进制摘要的单点实现（docs/12 §4.3 P1-8）。

use sha2::{Digest, Sha256};

/// `bytes` 的 SHA-256，64 个小写十六进制字符。
///
/// 与合并前三处实现逐字同形（`formula::sampling`、`table::fixture` 的局部
/// `digest_hex`，及 `table::run` 的两处内联），同算法同输出。
pub fn digest_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::digest_hex;

    /// 已知答案向量（FIPS 180-2 示例 + 空输入）：钉死"小写、64 字符、不截断"。
    #[test]
    fn digest_hex_matches_known_answers() {
        assert_eq!(
            digest_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// 旁证合一（P1-8 判据"三处调用同一函数"）：迁移后生产代码里只剩这一个实现；
    /// run 侧 `digest_file` 对同一字节必须给出与 `digest_hex` 相同的摘要，
    /// sampling/fixture 侧由其既有 manifest/digest 测试继续钉行为不变。
    #[test]
    fn the_migrated_call_sites_agree_on_identical_bytes() {
        let bytes: &[u8] = b"rapid-structure p1-8 corroboration vector";
        let temp = crate::test_support::TempDir::new("p1-8-digest-hex");
        let path = temp.path().join("blob.bin");
        std::fs::write(&path, bytes).expect("temp blob writable");
        let (len, via_run) = crate::evaluation::table::run::digest_file(&path)
            .expect("digest_file on a plain temp blob");
        assert_eq!(len, bytes.len() as u64);
        assert_eq!(via_run, digest_hex(bytes), "run side diverged");
    }
}
