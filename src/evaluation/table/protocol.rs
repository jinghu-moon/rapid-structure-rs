//! 表格评测协议（E8 / `docs/09-table-recognition-execution.md` §9）：
//! ε、网格规则、构造性上界扣除。
//!
//! 协议文档是 `docs/10-table-evaluation-protocol.md`。本模块是它的**可执行部分**：
//! 能被编译期常量、纯函数与单测钉住的部分放在代码里，论证与实测数字放在文档里。
//! 分成两半的理由是文档里的数字会腐烂而代码里的断言不会 —— `span_limits` 的教训
//! 就是"把从词表推出来的上限硬编码"会在换模型时静默失效
//! （见 [`crate::evaluation::table::fixture::span_limits`]）。
//!
//! # 三节
//!
//! 1. **ε（重复跑容差）** —— [`REPEAT_EPSILON`] 与 [`within_repeat_epsilon`]。
//! 2. **网格规则** —— [`AUTHORITATIVE_GRID_SOURCE`] 与 [`UNCOVERED_POSITION_RULE`]。
//! 3. **构造性上界扣除** —— [`UpperBound`] 与 [`classify_upper_bound`]。
//!
//! # 为什么 ε 是 0 而不是"一个小数"
//!
//! 任务清单的原话是"取 `Stats` 的 stddev 作下界，乘安全系数"。实测结果是
//! **stddev 恰好为 0**：同配置三次重跑的预测文件逐字节相同
//! （2,364,628 B，sha256 `2cfafeff…5288f0`）。于是"下界 × 安全系数"恒等于 0，
//! 安全系数这个乘数在数学上不产生任何作用。
//!
//! 与其保留一个不起作用的乘数，不如把 0 的**含义**写清楚：这里的 0 不是
//! "我们测不出抖动"，而是"**不存在抖动**"。机制上：
//!
//! - 输入形状被固定为 `[1,3,488,488]`（`TableStructureConfig::fix_input_shape`），
//!   因此不存在形状相关的分支；
//! - CPU 上的 ORT 对同一份图与同一份输入是确定的，且本 harness 每个样本单独跑
//!   （batch 恒为 1），没有批内归约顺序问题；
//! - 解码路径（[`crate::table::decode::argmax_structure`] → 结构树 → HTML）是纯函数，
//!   不含原子操作、不含哈希序迭代、不含时间/随机数。
//!
//! 因此**一次重复跑出现任何非零差异都不是噪声，而是缺陷**（非确定性）。
//! 这正是 [`within_repeat_epsilon`] 在 ε = 0 时退化为**逐位比较**而不是
//! `<= 0.0` 的原因：后者会把 `-0.0` 与 `0.0` 判为相等，而"相同"在这里是
//! 一个可以被字节级证据支持的主张。

use serde::Serialize;

/// 重复跑容差（TEDS 分数）。
///
/// 值见本模块文档：实测 stddev 恰好为 0，因此容差为 0，且 0 表示**逐位相同**。
pub const REPEAT_EPSILON: f64 = 0.0;

/// 模型结构解码循环的 trip 上限。
///
/// `models/table/slanet-plus.onnx` 的 `Loop` 节点携带常量
/// `assign_0.tmp_0.0 = 500`，每次迭代发射一个 token，遇到 `eos` 提前退出
/// （实测：零输入 → 27 步、随机输入 → 150 步，`L` 是动态维）。因此模型**最多**
/// 能产出 500 个结构 token，真值结构串更长的表格**在构造上**无法被复现。
///
/// # 为什么不从模型里探测
///
/// `character` 词表可以通过 ORT 的 `session.metadata_custom("character")` 读到，
/// 所以词表派生的上限（`rowspan` / `colspan`）是**探测**出来的；但 trip 上限是
/// 计算图内部的常量，ORT 不公开图结构，Rust 侧拿不到。
///
/// 因此这里用一个**自曝式**常量代替探测：报告里同时记录本数据集实测的
/// **最大结构 token 数**与这个上限，两者一旦接近，[`UpperBound::beyond_loop_trip_cap`]
/// 的计数就会从 0 开始增长 —— 常量过期会表现为"被扣除的样本变多"，
/// 而不是静默失效。
pub const LOOP_TRIP_CAP: usize = 500;

/// 逻辑网格的权威来源。
///
/// 数据集里有两套网格信息，实测结论是**只有一套可用**：
///
/// - [`GridSource::TokenStream`]：`val_CAPTIONS_STRUCTURE.json` 的结构 token 流。
///   顺序扫描 + `rowspan`/`colspan` 占位即可重建网格，是**构造性完整**的。
/// - [`GridSource::LabelMe`]：`cropped_datasets_1Tab_per_img/` 的逐图 LabelMe
///   `polygon` 标注。其 `label` 的 `row-col-rowspan-colspan-text` 字段只在
///   **约 84 %** 的样本上自洽（跨列超级单元格与其后单元格的起始列会错位半个跨度，
///   实测 5,995/7,104）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GridSource {
    /// PubTabNet 结构 token 流（`CAPLENS_STRUCTURE` / `CAPTIONS_STRUCTURE`）。
    TokenStream,
    /// 逐图 LabelMe 标注的位置字段。
    LabelMe,
}

/// 权威网格来源：**token 流**。
///
/// 依据（实测，7,104 个 val 样本）：以 token 流重建的单元格总数 = **161,925**，
/// 与 LabelMe 侧的单元格总数逐样本一致（7,104/7,104）；而
/// `(rowspan, colspan)` 多重集只在 token 流解释下吻合 7,104/7,104，在 LabelMe
/// 位置字段下 0/7,104。两套来源冲突时以 token 流为准，且**不剔除样本**。
pub const AUTHORITATIVE_GRID_SOURCE: GridSource = GridSource::TokenStream;

/// 网格里"没有任何单元格覆盖"的位置如何处理。
///
/// 顺序扫描 + 占位会留下空洞：某行的单元格总数（含跨度）小于该表的列数时，
/// 该行右侧的网格位置就没有单元格。实测 val 有 **24 / 7,104（0.34 %）** 张表
/// 存在空洞，合计 132 个位置，单表最多 27 个。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UncoveredPosition {
    /// 空洞**不产生单元格**：该位置在真值里就是空的，任何指标都不应该把它当成
    /// "漏了一个单元格"。
    KeepHole,
    /// 用最小包围盒铺满整行 —— 会**凭空发明**真值里不存在的单元格。
    FillWithBoundingBox,
    /// 把相邻单元格的跨度延伸过去 —— 同样会发明真值里不存在的 `colspan`。
    ExtendSpan,
    /// 剔除含空洞的样本 —— 会丢掉 0.34 % 的样本，且让"表短了一行"这件事
    /// 从指标里消失。
    DropSample,
}

/// 空洞处理规则：**保留空洞**（[`UncoveredPosition::KeepHole`]）。
///
/// 另外三个候选都会引入真值里不存在的信息或丢弃样本，因此被否决。这条规则对
/// **TEDS 没有影响**（TEDS 比较的是 HTML 树，而真值 HTML 本来就如实反映了短行），
/// 它约束的是未来若引入网格型指标（GriTS 一类）或单元格几何比对时的行为。
pub const UNCOVERED_POSITION_RULE: UncoveredPosition = UncoveredPosition::KeepHole;

/// 单个样本是否落在**构造性上界**内 —— 即模型无论多好都不可能做对。
///
/// 这类样本仍然计入头条指标（把它们从分数里删掉就是把"模型答错"变成
/// "样本不存在"），但**不进回归判据**：拿它们判断"这次改动是否让模型变差"
/// 是在判断一个模型无法控制的性质。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UpperBound {
    /// 真值含词表无法表达的 `rowspan` / `colspan`。
    ///
    /// 词表上限来自模型自己的 `character` metadata（`rowspan` / `colspan` 各
    /// 2..20），因此这个判据随模型自动更新，不是硬编码。
    pub span_beyond_vocabulary: bool,
    /// 真值结构串长度超过 [`LOOP_TRIP_CAP`]。
    pub beyond_loop_trip_cap: bool,
}

impl UpperBound {
    /// 是否属于被单列的上界样本。
    pub fn is_deducted(&self) -> bool {
        self.span_beyond_vocabulary || self.beyond_loop_trip_cap
    }

    /// 被单列的原因（按 `span` 优先），未被扣除时返回 `None`。
    pub fn reason(&self) -> Option<&'static str> {
        if self.span_beyond_vocabulary {
            Some("span_beyond_vocabulary")
        } else if self.beyond_loop_trip_cap {
            Some("beyond_loop_trip_cap")
        } else {
            None
        }
    }
}

/// 判定一个样本是否落在构造性上界内。
///
/// 两个输入都是**已经算好的事实**，不是估计值：`span_beyond_vocabulary` 来自
/// [`crate::evaluation::table::fixture::span_usage`] 对真值 HTML 的扫描，
/// `structure_tokens` 来自真值记录里的结构串长度。
pub fn classify_upper_bound(span_beyond_vocabulary: bool, structure_tokens: usize) -> UpperBound {
    UpperBound {
        span_beyond_vocabulary,
        beyond_loop_trip_cap: structure_tokens > LOOP_TRIP_CAP,
    }
}

/// "两次重复跑的结果相同"的判据。
///
/// [`REPEAT_EPSILON`] 为 0 时退化为**逐位比较**：`-0.0` 与 `0.0` 不算相同。
/// 这不是吹毛求疵 —— ε = 0 的主张是"不存在抖动"，而 `<= 0.0` 这种写法表达的
/// 是"差值小到可以忽略"，两者的强度不同。ε 非 0 时按普通数值容差比较。
pub fn within_repeat_epsilon(left: f64, right: f64) -> bool {
    if REPEAT_EPSILON == 0.0 {
        left.to_bits() == right.to_bits()
    } else {
        (left - right).abs() <= REPEAT_EPSILON
    }
}

/// 报告里的 `protocol` 段（静态部分）。
///
/// 评测报告自带协议本身，而不是要求读者去翻文档：一份 2027 年的报告要能自己说清
/// 它用的是哪套 ε、哪套网格规则、哪套上界扣除规则。这也让
/// `tools/build_table_teds_baseline.py` 能从报告里读 ε，而不是各自硬编码一份。
#[derive(Debug, Clone, Serialize)]
pub struct ProtocolSection {
    pub document: &'static str,
    pub repeat_epsilon: f64,
    pub repeat_epsilon_meaning: &'static str,
    pub authoritative_grid_source: GridSource,
    pub uncovered_position_rule: UncoveredPosition,
    pub loop_trip_cap: usize,
    pub loop_trip_cap_source: &'static str,
    pub upper_bound_policy: &'static str,
}

/// 构造报告里的 `protocol` 段。
pub fn protocol_section() -> ProtocolSection {
    ProtocolSection {
        document: "docs/10-table-evaluation-protocol.md",
        repeat_epsilon: REPEAT_EPSILON,
        repeat_epsilon_meaning: "0 means bit-identical, not approximately equal; a non-zero \
                                 difference between two same-configuration runs is a \
                                 nondeterminism defect, not noise",
        authoritative_grid_source: AUTHORITATIVE_GRID_SOURCE,
        uncovered_position_rule: UNCOVERED_POSITION_RULE,
        loop_trip_cap: LOOP_TRIP_CAP,
        loop_trip_cap_source: "the `Loop` node's trip-count constant in the ONNX graph; ORT does \
                               not expose graph internals, so this is a constant whose staleness \
                               shows up as a growing deducted-sample count",
        upper_bound_policy: "samples whose truth cannot be expressed by the model (span beyond \
                             the vocabulary, or a structure stream longer than the loop trip \
                             cap) are counted in the headline metric but excluded from the \
                             regression criterion",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GridSource, LOOP_TRIP_CAP, REPEAT_EPSILON, UNCOVERED_POSITION_RULE, UncoveredPosition,
        UpperBound, classify_upper_bound, protocol_section, within_repeat_epsilon,
    };

    #[test]
    fn repeat_epsilon_is_zero_because_the_runs_are_deterministic() {
        // 这个断言是**协议**，不是实现细节：它把"实测 stddev = 0"钉在编译产物里，
        // 于是"把 ε 调大一点让测试过"会先撞到这行。
        assert_eq!(REPEAT_EPSILON, 0.0);
    }

    #[test]
    fn zero_epsilon_compares_bits_not_values() {
        assert!(within_repeat_epsilon(0.706518, 0.706518));
        assert!(!within_repeat_epsilon(0.706518, 0.706519));
        // `<= 0.0` 会把这两个判为相等，而它们不是同一份结果。
        assert!(!within_repeat_epsilon(0.0, -0.0));
    }

    #[test]
    fn a_positive_epsilon_would_fall_back_to_a_numeric_tolerance() {
        // ε 非 0 时不该再用逐位比较。这里不修改常量，而是直接验证公式本身，
        // 避免测试依赖"未来某个 commit 把 ε 改成非 0"。
        let epsilon = 0.01_f64;
        let close = |left: f64, right: f64| (left - right).abs() <= epsilon;
        assert!(close(0.7065, 0.7100));
        assert!(!close(0.7065, 0.7200));
    }

    #[test]
    fn the_grid_source_is_the_token_stream() {
        assert_eq!(super::AUTHORITATIVE_GRID_SOURCE, GridSource::TokenStream);
    }

    #[test]
    fn holes_are_kept_rather_than_filled_or_dropped() {
        assert_eq!(UNCOVERED_POSITION_RULE, UncoveredPosition::KeepHole);
    }

    #[test]
    fn the_loop_cap_is_the_measured_graph_constant() {
        // 模型图里的常量就是 500。改这个数字必须同时改文档里的实测依据。
        assert_eq!(LOOP_TRIP_CAP, 500);
    }

    #[test]
    fn a_table_at_the_cap_is_not_yet_deducted() {
        // 边界：恰好 500 个 token 仍然能被 500 步循环产出，因此**不**扣除。
        let at_cap = classify_upper_bound(false, LOOP_TRIP_CAP);
        assert!(!at_cap.beyond_loop_trip_cap);
        assert!(!at_cap.is_deducted());
        assert_eq!(at_cap.reason(), None);

        let past_cap = classify_upper_bound(false, LOOP_TRIP_CAP + 1);
        assert!(past_cap.beyond_loop_trip_cap);
        assert!(past_cap.is_deducted());
        assert_eq!(past_cap.reason(), Some("beyond_loop_trip_cap"));
    }

    #[test]
    fn a_span_beyond_the_vocabulary_is_deducted_and_reported_first() {
        let both = classify_upper_bound(true, LOOP_TRIP_CAP + 1);
        assert!(both.is_deducted());
        // 两个原因同时成立时报 span：它是**真值侧**的限制，与模型质量无关。
        assert_eq!(both.reason(), Some("span_beyond_vocabulary"));
    }

    #[test]
    fn an_ordinary_table_is_not_deducted() {
        let ordinary = classify_upper_bound(false, 48);
        assert_eq!(
            ordinary,
            UpperBound {
                span_beyond_vocabulary: false,
                beyond_loop_trip_cap: false,
            }
        );
        assert!(!ordinary.is_deducted());
    }

    #[test]
    fn the_protocol_section_carries_its_own_document_reference() {
        let section = protocol_section();
        assert_eq!(section.document, "docs/10-table-evaluation-protocol.md");
        assert_eq!(section.repeat_epsilon, REPEAT_EPSILON);
        assert_eq!(section.loop_trip_cap, LOOP_TRIP_CAP);
        assert_eq!(section.authoritative_grid_source, GridSource::TokenStream);
        assert_eq!(section.uncovered_position_rule, UncoveredPosition::KeepHole);
        // 序列化后必须仍然自描述：报告是长期产物，不能只留一个裸数字。
        let value = serde_json::to_value(&section).expect("protocol section is serializable");
        assert_eq!(value["authoritative_grid_source"], "token_stream");
        assert_eq!(value["uncovered_position_rule"], "keep_hole");
        assert!(value["upper_bound_policy"].is_string());
    }
}
