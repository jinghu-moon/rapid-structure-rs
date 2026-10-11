//! SLANet-plus 表格结构模型 IO 契约的**唯一**实现。
//!
//! 与公式域同构（见 [`crate::formula::contract`]）：探针（[`TableStructureModelInfo::probe`]）
//! 与将来的运行时 session 必须共用同一份校验，否则会出现"探针成功、session 失败"。
//!
//! # 契约事实（`models/table/slanet-plus.onnx` 实测）
//!
//! - 输入：单输入 `x`，`FLOAT [N, 3, H, W]`，**四个维度全是动态的**；
//! - 输出：**两路** `FLOAT` rank-3 ——
//!   `save_infer_model/scale_0.tmp_0` `[N, L, 8]`（单元格框）与
//!   `save_infer_model/scale_1.tmp_0` `[N, L, 50]`（结构 token 概率）；
//! - `character` metadata：换行分隔的 48 个 token（解析见 [`crate::table::metadata`]）。
//!
//! # 为什么输入空间维不要求固定
//!
//! 公式域要求 `[N,1,384,384]` 固定，因为公式预处理总是产出那个形状。表格域不同：
//! 真实模型四个维度全动态，且 `L`（解码长度）**必须**保持动态 ——
//! 固定成 500 会强制跑满整个解码循环，把最坏情况当成常态
//! （见 `docs/09-table-recognition-execution.md` §6）。
//! 因此这里只要求 rank、通道数与 dtype。
//!
//! 输出末维则**必须**固定：`8` 是 `[x1,y1,x2,y2,score₁,score₂,cls,?]` 的宽度，
//! `50` 是词表宽度，二者都不是"模型可选"的参数。
//!
//! # 形状固定（在 [`TableStructureConfig`] 里而不是常量里）
//!
//! 输入 H/W 在运行时用 ONNX Runtime 的 free-dimension override 固定为画布边长，
//! **模型文件不改**。这不是"顺手调参"：H/W 动态时 ORT 无法把 `Resize` / `Shape` /
//! `Reshape` 一路折叠掉，也无法为激活值做静态的缓冲规划。`L` 则**必须**保持动态 ——
//! 它是自回归解码长度，实测零输入 27 步、随机输入 150 步，而 `Loop` 的 trip 上限常量是
//! 500：固定成 500 等于把最坏情况当成常态。
//!
//! 该断言由 [`TableStructureSession::open`] 的**后置条件检查**强制执行：ORT 对无法解析的
//! override 名字不报错、只是不生效，所以"已经固定了"必须重新探针一次来证明。

use std::path::Path;

use ndarray::{ArrayView3, ArrayView4};
use ort::value::TensorElementType;

use crate::{
    config::RuntimeConfig,
    error::{RapidOcrError, Result},
    runtime::{
        contracts::{
            DYNAMIC_DIM, ModelIoProbe, require_dynamic_dim_symbol, require_outputs,
            require_single_input, require_tensor,
        },
        session::{DimensionOverride, OrtSession},
    },
    table::metadata::{parse_character_metadata, validate_vocabulary_width},
};

/// 输入契约：单输入 `FLOAT [N, 3, H, W]`。
pub const TABLE_STRUCTURE_INPUT_RANK: usize = 4;
pub const TABLE_STRUCTURE_INPUT_CHANNELS: i64 = 3;

/// 输入 H / W 在 `[N, C, H, W]` 里的下标。
pub const TABLE_STRUCTURE_HEIGHT_DIM: usize = 2;
pub const TABLE_STRUCTURE_WIDTH_DIM: usize = 3;

/// SLANet-plus 的训练画布边长（正方形）。见
/// `docs/09-table-recognition-execution.md` §6：它是 [`TableStructureConfig::input_side_len`]
/// 的默认值，不是唯一合法值。
pub const TABLE_STRUCTURE_SIDE_LEN: usize = 488;

/// 单元格框输出：`FLOAT [N, L, 8]`。
pub const TABLE_STRUCTURE_BOX_OUTPUT: &str = "save_infer_model/scale_0.tmp_0";
pub const TABLE_STRUCTURE_BOX_RANK: usize = 3;
pub const TABLE_STRUCTURE_BOX_LAST_DIM: i64 = 8;

/// 结构 token 概率输出：`FLOAT [N, L, 50]`。
pub const TABLE_STRUCTURE_TOKEN_OUTPUT: &str = "save_infer_model/scale_1.tmp_0";
pub const TABLE_STRUCTURE_TOKEN_RANK: usize = 3;
pub const TABLE_STRUCTURE_TOKEN_LAST_DIM: i64 = 50;

/// 两路输出，按契约要求的顺序。
pub const TABLE_STRUCTURE_OUTPUTS: [&str; 2] =
    [TABLE_STRUCTURE_BOX_OUTPUT, TABLE_STRUCTURE_TOKEN_OUTPUT];

/// 通过校验的表格结构模型签名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStructureContract {
    pub input_name: String,
    pub box_output_name: String,
    pub structure_output_name: String,
}

/// 校验表格结构模型 IO 契约；探针与运行时 session 共用。
///
/// 只校验计算图签名，**不读 metadata** —— 需要 metadata 的调用方用
/// [`TableStructureModelInfo::probe`]，或自行读 `character` 后交给
/// [`crate::table::metadata::parse_character_metadata`]。
pub fn validate_table_structure_contract(
    probe: &ModelIoProbe,
    model_path: &Path,
) -> Result<TableStructureContract> {
    let input = require_single_input(probe, model_path, "table structure")?;
    require_tensor(
        input,
        "input",
        TABLE_STRUCTURE_INPUT_RANK,
        TensorElementType::Float32,
        model_path,
    )?;
    // 通道维必须固定为 3：SLANet 的预处理永远产出 3 通道，接受动态通道不会带来
    // 任何额外能力，只会让探针结论失去可操作性。
    crate::runtime::contracts::require_fixed_dim(
        input,
        "input",
        1,
        TABLE_STRUCTURE_INPUT_CHANNELS,
        model_path,
    )?;

    let outputs = require_outputs(
        probe,
        model_path,
        "table structure",
        &TABLE_STRUCTURE_OUTPUTS,
    )?;
    // `box` 是 Rust 2024 的保留字，因此这里用 `boxes` / `structure` 命名两个 spec。
    let (boxes, structure) = (outputs[0], outputs[1]);
    require_tensor(
        boxes,
        "output",
        TABLE_STRUCTURE_BOX_RANK,
        TensorElementType::Float32,
        model_path,
    )?;
    require_tensor(
        structure,
        "output",
        TABLE_STRUCTURE_TOKEN_RANK,
        TensorElementType::Float32,
        model_path,
    )?;
    crate::runtime::contracts::require_fixed_dim(
        boxes,
        "output",
        2,
        TABLE_STRUCTURE_BOX_LAST_DIM,
        model_path,
    )?;
    crate::runtime::contracts::require_fixed_dim(
        structure,
        "output",
        2,
        TABLE_STRUCTURE_TOKEN_LAST_DIM,
        model_path,
    )?;

    Ok(TableStructureContract {
        input_name: input.name.clone(),
        box_output_name: boxes.name.clone(),
        structure_output_name: structure.name.clone(),
    })
}

/// 表格结构推理的形状策略。
///
/// **为什么是配置而不是常量**：`input_side_len` 必须能跟着预处理用的画布尺寸走，
/// `fix_input_shape` 是策略（是否把图按静态形状编译），两者都不是模型事实 ——
/// 写死在乘法或 `if` 里，就会在换一个导出模型或换一个画布尺寸时沉默地失配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStructureConfig {
    /// 输入正方形画布的边长。
    pub input_side_len: usize,
    /// 是否用 ONNX Runtime 的 free-dimension override 把输入 H/W 固定为 `input_side_len`。
    ///
    /// 关掉它只有两个理由：与固定前做对照实验，或将来某个 provider 不接受静态形状。
    pub fix_input_shape: bool,
    /// 本会话自己的 ORT intra 线程数；`None` = 沿用传入的 [`RuntimeConfig`]。
    ///
    /// # 为什么表格会话需要一只自己的旋钮
    ///
    /// ORT 的 `session.intra_op.allow_spinning` 默认为 `"1"`：intra 线程池里的空闲线程
    /// **自旋**而不是休眠。进程里已经有页级 det + rec（公式路径还有更多）若干个会话，
    /// 每个都按 [`crate::runtime::profile::auto_tuned_thread_budget`] 拿满核数；表格会话再拿
    /// 一份，得到的是若干个都在自旋的池子争抢同一批核，而不是更多算力。
    ///
    /// 同时 SLANet-plus 只有 830 个节点（77 个 `Conv`、73 个 `HardSwish`），batch 固定为 1 ——
    /// 单张图能提供的并行度本来就有限，"线程越多越快"在这里没有第一性依据。
    ///
    /// 但**有依据的怀疑不等于有依据的结论**：这只旋钮的默认值是 `None`（继承），
    /// 是否值得改由 `docs/09-table-recognition-execution.md` §13.3 的实测判据决定 ——
    /// p50 必须超出 ±10–15 % 噪声带才允许提交，落在带内就回退。
    pub intra_threads: Option<usize>,
}

impl Default for TableStructureConfig {
    fn default() -> Self {
        Self {
            input_side_len: TABLE_STRUCTURE_SIDE_LEN,
            fix_input_shape: true,
            intra_threads: None,
        }
    }
}

impl TableStructureConfig {
    /// 把本配置的线程要求叠加到引擎的 [`RuntimeConfig`] 上。
    ///
    /// `intra_threads = Some(n)` 时**覆盖**传入配置里的线程设置（本会话自己的要求优先于
    /// 引擎的全局值），并交给 [`RuntimeConfig::with_pinned_intra_threads`] 写成自洽的
    /// 显式请求；`None` 时原样返回传入配置，因此"不设置"在执行侧是真的不设置。
    pub fn effective_runtime_config(&self, runtime_cfg: &RuntimeConfig) -> RuntimeConfig {
        match self.intra_threads {
            Some(threads) => runtime_cfg.with_pinned_intra_threads(threads),
            None => runtime_cfg.clone(),
        }
    }

    /// 从**已经探到**的 IO 推出这次会话要施加的 override。
    ///
    /// 符号名一律从 `probe` 读（不是常量）：`paddle2onnx` 生成的是
    /// `DynamicDimension.<n>` 这类按维度出现顺序命名的位置名，重转一次就会变，
    /// 而写错的名字在 ORT 里**不会报错**，只是不生效。
    ///
    /// 维度已经是目标值时返回空列表（不施加冗余 override）；已经是**别的**定长值时直接报错 ——
    /// override 改不了定长维度，静默放行只会让"固定了形状"变成假声明。
    pub fn dimension_overrides(
        &self,
        probe: &ModelIoProbe,
        model_path: &Path,
    ) -> Result<Vec<DimensionOverride>> {
        if !self.fix_input_shape {
            return Ok(Vec::new());
        }
        let input = require_single_input(probe, model_path, "table structure")?;
        let side = i64::try_from(self.input_side_len).map_err(|error| {
            RapidOcrError::Config(format!("input_side_len does not fit in i64: {error}"))
        })?;
        let mut overrides = Vec::with_capacity(2);
        for dim_index in [TABLE_STRUCTURE_HEIGHT_DIM, TABLE_STRUCTURE_WIDTH_DIM] {
            let actual = *input.dims.get(dim_index).ok_or_else(|| {
                RapidOcrError::Config(format!(
                    "model input `{}` is missing dim index {dim_index} (dims={:?}, model={})",
                    input.name,
                    input.dims,
                    model_path.display()
                ))
            })?;
            if actual == side {
                continue;
            }
            if actual != DYNAMIC_DIM {
                return Err(RapidOcrError::Config(format!(
                    "table structure input `{}` dim {dim_index} is fixed to {actual}, but `input_side_len` is {side}; \
                     a fixed dimension cannot be overridden (dims={:?}, model={})",
                    input.name,
                    input.dims,
                    model_path.display()
                )));
            }
            let symbol = require_dynamic_dim_symbol(input, "input", dim_index, model_path)?;
            overrides.push(DimensionOverride::new(symbol, side));
        }
        Ok(overrides)
    }

    /// 后置条件：会话**建好之后**重新探针到的输入 H/W 必须等于 `input_side_len`。
    ///
    /// 这一步不能省。ORT 的 `AddFreeDimensionOverrideByName` 对无法解析的名字（拼错、
    /// 换过一次导出、符号名其实不存在）不返回错误，只是不生效 —— 于是"我们已经固定了形状"
    /// 从一句可验证的陈述退化成一个没人检查的声明，而性能差异会被记到别的原因上。
    pub fn require_applied(&self, probe: &ModelIoProbe, model_path: &Path) -> Result<()> {
        if !self.fix_input_shape {
            return Ok(());
        }
        let input = require_single_input(probe, model_path, "table structure")?;
        let side = i64::try_from(self.input_side_len).map_err(|error| {
            RapidOcrError::Config(format!("input_side_len does not fit in i64: {error}"))
        })?;
        for dim_index in [TABLE_STRUCTURE_HEIGHT_DIM, TABLE_STRUCTURE_WIDTH_DIM] {
            let actual = input.dims.get(dim_index).copied().unwrap_or(DYNAMIC_DIM);
            if actual != side {
                return Err(RapidOcrError::Config(format!(
                    "table structure input `{}` dim {dim_index} is {actual} after opening the session with \
                     `fix_input_shape = true`; the free-dimension override did not take effect \
                     (expected {side}, dims={:?}, model={})",
                    input.name,
                    input.dims,
                    model_path.display()
                )));
            }
        }
        Ok(())
    }
}

/// 一次结构推理的两路输出视图。
///
/// 视图借用 ONNX Runtime 的输出缓冲，**生命周期不得逃逸** [`TableStructureSession::run_with`]
/// 的回调：需要留存的量必须在回调内 `to_owned()`。
#[derive(Debug)]
pub struct TableStructureOutputs<'a> {
    /// `[N, L, 8]` 单元格框（对应参考实现的 `pred[0]`）。
    pub boxes: ArrayView3<'a, f32>,
    /// `[N, L, 50]` 结构 token 概率（对应参考实现的 `pred[1]`）。
    pub tokens: ArrayView3<'a, f32>,
}

/// 表格结构推理会话：契约 + 形状策略 + 两路输出的 typed 访问。
///
/// 契约校验与 [`TableStructureModelInfo::probe`] 共用
/// [`validate_table_structure_contract`]，因此不存在"探针通过、会话失败"的分叉。
#[derive(Debug)]
pub struct TableStructureSession {
    session: OrtSession,
    contract: TableStructureContract,
    config: TableStructureConfig,
    /// 建会话**之后**探到的输入维度；它让 [`TableStructureConfig::require_applied`]
    /// 的后置条件可被测试直接观察，而不是只能靠"没报错"推断。
    input_dims: Vec<i64>,
    /// 建会话时实际下发给 ORT 的 `(intra, inter)` 线程数，见
    /// [`TableStructureSession::session_threads`]。
    session_threads: (Option<usize>, Option<usize>),
    /// 模型 metadata 里的 `character` 词表，见 [`TableStructureSession::vocabulary`]。
    vocabulary: crate::table::decode::StructureVocabulary,
}

impl TableStructureSession {
    /// 从**已验证凭证**打开表格结构会话（审核点 1/2：与文本、公式域同一契约）。
    ///
    /// 完整性校验（存在性、可选的期望摘要）与内容绑定都发生在凭证构造点：调用方用
    /// [`crate::table::registration::verified_registered_model`]（显式期望优先、否则取登记）
    /// 或 [`crate::model_verify::VerifiedModel::verify`] 自己解析“期望从哪来”。探针会话与
    /// 正式会话都从凭证的**内容快照**字节打开（`open_verified`）：同一次读取、同一个
    /// 对象，“校验与打开之间被替换”的窗口在表格路径（曾经最长）上不存在；而“拿不到
    /// 凭证就建不了会话”在类型上成立。
    pub fn open(
        model: &crate::model_verify::VerifiedModel,
        runtime_cfg: &RuntimeConfig,
        config: &TableStructureConfig,
    ) -> Result<Self> {
        let model_path = model.path();

        // 线程要求在**开任何会话之前**就落到运行时配置上：探针会话与正式会话必须是同一套
        // 线程设置，否则"这个会话用了几个线程"会有两个答案，而探针那次的开销会被记到别处。
        let runtime_cfg = config.effective_runtime_config(runtime_cfg);

        // 先开一个"什么都不固定"的会话：override 只能按符号名寻址，而符号名只有模型自己知道，
        // 所以必须先探一次。这一次会话在拿到符号名之后立即释放。
        // 两次会话消费同一份凭证字节，不再第二次开卷路径。
        let probe_session = OrtSession::open_verified(model, &runtime_cfg, &[])?;
        let probe = probe_session.probe_io()?;
        let contract = validate_table_structure_contract(&probe, model_path)?;
        // 词表与契约在**同一次**探针会话里取出：解码需要 `character` 词表，而它和
        // 输出末维的交叉校验（metadata vs 计算图，两个独立来源）正是"模型与词表配套"
        // 的唯一证据。放在这里而不是让调用方再 probe 一次，是为了让页面管线**只开一次**
        // 会话就能拿到解码所需的全部信息。
        let character_metadata = probe_session.metadata_custom("character")?.ok_or_else(|| {
            RapidOcrError::Config(format!(
                "table structure model is missing `character` metadata; refusing to fall back to \
                 the ordinary OCR dictionary (model={})",
                model_path.display()
            ))
        })?;
        let character_tokens = parse_character_metadata(&character_metadata)?;
        // 末维已由 `validate_table_structure_contract` 的 `require_fixed_dim` 钉在
        // `TABLE_STRUCTURE_TOKEN_LAST_DIM` 上，所以这里比对常量与比对计算图是同一件事。
        validate_vocabulary_width(
            &character_tokens,
            TABLE_STRUCTURE_TOKEN_LAST_DIM as usize,
            model_path,
        )?;
        let vocabulary = crate::table::decode::StructureVocabulary::new(character_tokens)?;
        let overrides = config.dimension_overrides(&probe, model_path)?;
        drop(probe_session);

        let session = OrtSession::open_verified(model, &runtime_cfg, &overrides)?;
        let actual = session.probe_io()?;
        config.require_applied(&actual, model_path)?;
        let input_dims = actual.inputs[0].dims.clone();
        let session_threads = session.session_threads();

        Ok(Self {
            session,
            contract,
            config: config.clone(),
            input_dims,
            session_threads,
            vocabulary,
        })
    }

    /// 模型的 `character` 词表（已与输出末维交叉校验，见 [`TableStructureSession::open`]）。
    ///
    /// 解码器（[`crate::table::decode`]）需要它，而它只能从模型 metadata 得到；
    /// 把它挂在会话上意味着"用这个会话解码"不需要第二个信息来源。
    pub fn vocabulary(&self) -> &crate::table::decode::StructureVocabulary {
        &self.vocabulary
    }

    /// 本会话**实际下发给 ONNX Runtime** 的 `(intra, inter)` 线程数。
    ///
    /// `None` 表示该线程数没有被配置（没有调用对应的 `with_*_threads`），ORT 用自己的默认值，
    /// 不表示 0 线程。它是 [`TableStructureConfig::intra_threads`] 这只旋钮的**后置条件**：
    /// 没有它，"配置里写了 4 个线程"就只是一句没人核对过的声明。
    pub fn session_threads(&self) -> (Option<usize>, Option<usize>) {
        self.session_threads
    }

    /// 建这个会话时交给 ONNX Runtime 的 EP 链，以及它是否发生了回退。
    ///
    /// 与 [`crate::runtime::provider::ProviderResolution::selected_ep`] 的告诫相同：这只描述
    /// **我们交出去的那条链**，不描述模型逐节点最终在哪个 EP 上执行，因此**不能当作加速
    /// 证据**。它在这里的用途是另一个：DirectML 的 per-EP 构建选项（`DisableMemPattern` +
    /// `ORT_SEQUENTIAL`）与 opset 上限都是**按 EP 分支**的，于是"这个会话走的是哪条分支"
    /// 必须可以被调用方与测试直接观察到，而不是只能从"没报错"推断。
    pub fn provider_resolution(&self) -> crate::runtime::provider::ProviderResolution {
        self.session.provider_resolution()
    }

    /// 模型的 ONNX opset；后端不支持 Model Editor API 时为 `None`。
    ///
    /// 见 [`crate::runtime::session::require_direct_ml_opset`]：DirectML 要求 opset ≤ 20。
    pub fn onnx_opset(&self) -> Option<u32> {
        self.session.onnx_opset()
    }

    pub fn contract(&self) -> &TableStructureContract {
        &self.contract
    }

    pub fn config(&self) -> &TableStructureConfig {
        &self.config
    }

    /// 建会话之后实测到的输入维度。
    pub fn input_dims(&self) -> &[i64] {
        &self.input_dims
    }

    /// 跑一次结构推理，按**契约里的名字**取回两路输出。
    ///
    /// 用名字查找而不是用下标：`output_names` 是模型声明顺序，把它当成契约顺序会让一次
    /// 重新导出（两路输出换序）变成静默的错误。
    pub fn run_with<T, F>(&mut self, input: ArrayView4<'_, f32>, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(TableStructureOutputs<'a>) -> Result<T>,
    {
        let box_name = self.contract.box_output_name.clone();
        let structure_name = self.contract.structure_output_name.clone();
        self.session.run_all_f32_view_with(input, |outputs| {
            let pick = |name: &str| -> Result<ArrayView3<'_, f32>> {
                let view = outputs
                    .iter()
                    .find(|(output_name, _)| *output_name == name)
                    .map(|(_, view)| view.clone())
                    .ok_or_else(|| {
                        RapidOcrError::Decode(format!(
                            "table structure run result is missing output `{name}`"
                        ))
                    })?;
                view.into_dimensionality::<ndarray::Ix3>().map_err(|error| {
                    RapidOcrError::Decode(format!(
                        "unexpected rank for table structure output `{name}`: expected rank 3: {error}"
                    ))
                })
            };
            let boxes = pick(&box_name)?;
            let tokens = pick(&structure_name)?;
            f(TableStructureOutputs { boxes, tokens })
        })
    }
}

/// 表格结构模型签名探针结果。
#[derive(Debug, Clone)]
pub struct TableStructureModelInfo {
    pub model_path: String,
    pub input_name: String,
    pub input_dims: Vec<i64>,
    pub box_output_name: String,
    pub structure_output_name: String,
    /// 结构 logits 末维（= 类别数）。
    pub structure_last_dim: usize,
    /// `character` metadata 的原始 `character` 词表（不含 `sos` / `eos`）。
    pub character_tokens: Vec<String>,
}

impl TableStructureModelInfo {
    /// 探针入口：加载模型、校验签名与 `character` metadata，不执行推理。
    ///
    /// metadata 校验复用 [`parse_character_metadata`]，并把词表宽度与计算图末维
    /// **交叉校验** —— 两个数字来源独立（metadata vs 计算图），因此能互相证伪。
    ///
    /// 与 [`TableStructureSession::open`] 一样在打开会话**之前**做完整性校验，并把会话
    /// 从凭证的**内容快照**打开（审核点 1）。这里没有配置可以传，所以用登记里的期望
    /// 哈希（见 [`crate::table::registration`]）；模型旁边没有清单时校验退化为
    /// “无期望凭证”（存在性照查、内容照绑定），而不是"跳过校验"。
    pub fn probe(model_path: &Path, runtime_cfg: &RuntimeConfig) -> Result<Self> {
        if !model_path.is_file() {
            return Err(RapidOcrError::FileNotFound(model_path.to_path_buf()));
        }
        let model = crate::table::registration::verified_registered_model(model_path, None)?;

        let session = OrtSession::open_verified(&model, runtime_cfg, &[])?;
        let io = session.probe_io()?;
        let contract = validate_table_structure_contract(&io, model_path)?;
        let input = &io.inputs[0];

        let character_metadata = session.metadata_custom("character")?.ok_or_else(|| {
            RapidOcrError::Config(format!(
                "table structure model is missing `character` metadata; refusing to fall back to \
                 the ordinary OCR dictionary (model={})",
                model_path.display()
            ))
        })?;
        let character_tokens = parse_character_metadata(&character_metadata)?;
        validate_vocabulary_width(
            &character_tokens,
            TABLE_STRUCTURE_TOKEN_LAST_DIM as usize,
            model_path,
        )?;

        Ok(Self {
            model_path: model_path.display().to_string(),
            input_name: contract.input_name,
            input_dims: input.dims.clone(),
            box_output_name: contract.box_output_name,
            structure_output_name: contract.structure_output_name,
            structure_last_dim: TABLE_STRUCTURE_TOKEN_LAST_DIM as usize,
            character_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::runtime::contracts::TensorSpec;

    fn fixture(name: &str) -> PathBuf {
        crate::test_support::fixture_dir("table-onnx").join(name)
    }

    fn rt() -> RuntimeConfig {
        RuntimeConfig::default()
    }

    /// 测试构造凭证的公共入口：会话只能从凭证打开，路径形态的 `open` 已不存在（审核点 2）。
    fn cred(path: &std::path::Path) -> crate::model_verify::VerifiedModel {
        crate::model_verify::VerifiedModel::verify(path, None).expect("test credential")
    }

    fn probe_fixture(name: &str) -> Result<TableStructureContract> {
        let path = fixture(name);
        let session = OrtSession::open_unchecked(&path, &rt(), &[]).expect("fixture must load");
        let io = session.probe_io().expect("fixture must probe");
        validate_table_structure_contract(&io, &path)
    }

    #[test]
    fn table_structure_contract_accepts_the_ok_fixture() {
        let contract =
            probe_fixture("table_ok.onnx").expect("ok fixture must satisfy the contract");
        assert_eq!(contract.input_name, "x");
        assert_eq!(contract.box_output_name, TABLE_STRUCTURE_BOX_OUTPUT);
        assert_eq!(contract.structure_output_name, TABLE_STRUCTURE_TOKEN_OUTPUT);
    }

    #[test]
    fn table_structure_requires_two_outputs() {
        let error = probe_fixture("table_single_output.onnx").expect_err("one output is not two");
        let message = error.to_string();
        assert!(message.contains("table structure"), "{message}");
        assert!(message.contains("1 outputs"), "{message}");
        assert!(
            message.contains(TABLE_STRUCTURE_TOKEN_OUTPUT),
            "the missing output must be named: {message}"
        );
        assert!(message.contains("table_single_output.onnx"), "{message}");
    }

    /// 输出数与 dtype 全对、只有名字错的模型**必须**被拒绝。
    ///
    /// 这条 fixture 存在的意义：一个只数输出个数、不比对名字的实现会静默通过它，
    /// 然后在运行时把框当成结构概率去解码。
    ///
    /// 真实输出名 `boxes` / `structures` 必须出现在错误消息里 —— 否则使用者只知道
    /// "缺了 `save_infer_model/scale_0.tmp_0`"，却不知道模型实际给的是什么。
    #[test]
    fn table_structure_requires_the_exported_output_names() {
        let error = probe_fixture("table_renamed_output.onnx").expect_err("renamed outputs");
        let message = error.to_string();
        assert!(message.contains("output mismatch"), "{message}");
        assert!(message.contains(TABLE_STRUCTURE_BOX_OUTPUT), "{message}");
        assert!(message.contains("boxes"), "{message}");
        assert!(message.contains("structures"), "{message}");
        assert!(message.contains("table_renamed_output.onnx"), "{message}");
    }

    /// 两路契约输出都齐全、但**额外**多声明一路输出时也必须被拒绝。
    ///
    /// 这与 `table_renamed_output.onnx` 走的是**不同的错误分支**（多余输出 vs 缺失输出）。
    /// 若不拒绝多余输出，一个被重新导出、语义已改的模型会带着未经验证的第三路输出
    /// 通过契约 —— "恰好这一组"里的"恰好"就落空了。
    #[test]
    fn table_structure_rejects_undeclared_extra_outputs() {
        let error = probe_fixture("table_extra_output.onnx").expect_err("extra output");
        let message = error.to_string();
        assert!(message.contains("unexpected"), "{message}");
        assert!(message.contains("extra"), "{message}");
        assert!(message.contains("3 outputs"), "{message}");
        assert!(message.contains("table_extra_output.onnx"), "{message}");
    }

    #[test]
    fn table_structure_requires_float32_outputs() {
        let error = probe_fixture("table_output_dtype.onnx").expect_err("int64 structure output");
        let message = error.to_string();
        assert!(message.contains("FLOAT32"), "{message}");
        assert!(message.contains("INT64"), "{message}");
    }

    #[test]
    fn table_structure_requires_the_contract_last_dims() {
        let error = probe_fixture("table_output_last_dim.onnx").expect_err("last dim 51");
        let message = error.to_string();
        assert!(message.contains("expected 50, got 51"), "{message}");
    }

    #[test]
    fn table_structure_requires_a_single_float_rank4_input() {
        let multi = probe_fixture("table_multi_input.onnx").expect_err("two inputs");
        assert!(multi.to_string().contains("exactly one input"), "{multi}");

        let int64 = probe_fixture("table_input_int64.onnx").expect_err("int64 input");
        assert!(int64.to_string().contains("FLOAT32"), "{int64}");

        let rank3 = probe_fixture("table_input_rank3.onnx").expect_err("rank-3 input");
        assert!(rank3.to_string().contains("rank mismatch"), "{rank3}");
    }

    /// 多输出 API 必须一次取回**两路**张量，且形状分别是 `(N,L,8)` 与 `(N,L,50)`。
    ///
    /// 这条测试锁住 `run_arrayd_view_with`「只取第一个输出」的静默错误：
    /// 若退回单输出 API，结构概率那一路会被无声丢掉。
    #[test]
    fn table_structure_returns_both_outputs_in_one_run() {
        let path = fixture("table_ok.onnx");
        let mut session = OrtSession::open_unchecked(&path, &rt(), &[]).expect("fixture must load");
        let input = ndarray::Array4::<f32>::zeros((2, 3, 488, 488));

        let observed = session
            .run_all_f32_view_with(input.view(), |outputs| {
                Ok(outputs
                    .iter()
                    .map(|(name, view)| ((*name).to_string(), view.shape().to_vec()))
                    .collect::<Vec<_>>())
            })
            .expect("two-output run must succeed");

        assert_eq!(observed.len(), 2, "both outputs must be observable");
        assert_eq!(
            observed[0],
            (
                TABLE_STRUCTURE_BOX_OUTPUT.to_string(),
                vec![2, 4, TABLE_STRUCTURE_BOX_LAST_DIM as usize]
            )
        );
        assert_eq!(
            observed[1],
            (
                TABLE_STRUCTURE_TOKEN_OUTPUT.to_string(),
                vec![2, 4, TABLE_STRUCTURE_TOKEN_LAST_DIM as usize]
            )
        );
    }

    #[test]
    fn character_metadata_must_be_present() {
        let error = TableStructureModelInfo::probe(&fixture("table_metadata_absent.onnx"), &rt())
            .expect_err("metadata is required");
        assert!(error.to_string().contains("character"), "{error}");
    }

    #[test]
    fn character_metadata_must_not_be_json() {
        let error = TableStructureModelInfo::probe(&fixture("table_metadata_json.onnx"), &rt())
            .expect_err("formula-style JSON metadata must be rejected");
        let message = error.to_string();
        assert!(message.contains("newline-separated"), "{message}");
        assert!(message.contains("JSON"), "{message}");
    }

    #[test]
    fn ok_fixture_probes_the_full_signature_and_vocabulary() {
        let info = TableStructureModelInfo::probe(&fixture("table_ok.onnx"), &rt())
            .expect("ok fixture must probe");
        assert!(
            info.model_path.ends_with("table_ok.onnx"),
            "the probe must record which model it came from, got {}",
            info.model_path
        );
        assert_eq!(info.input_name, "x");
        assert_eq!(info.input_dims.len(), TABLE_STRUCTURE_INPUT_RANK);
        assert_eq!(info.structure_last_dim, 50);
        assert_eq!(info.character_tokens.len(), 48);
        assert_eq!(info.character_tokens[6], "<td");
        assert_eq!(info.character_tokens[47], "<td></td>");
    }

    #[test]
    fn missing_model_returns_located_error() {
        let error = TableStructureModelInfo::probe(&fixture("does_not_exist.onnx"), &rt())
            .expect_err("must fail");
        assert!(matches!(error, RapidOcrError::FileNotFound(_)), "{error}");
    }

    // ---------------------------------------------------------------- 形状固定

    /// 维度固定必须**从探针读到的符号名**推出，不能写死 `DynamicDimension.<n>`。
    ///
    /// 合成 fixture 用的是语义符号名 `'H'` / `'W'`，真实模型用的是 `paddle2onnx` 生成的
    /// `DynamicDimension.<n>`。写死其中任何一个，换一次导出就会命名不符 —— 而 ORT 对
    /// 无法解析的名字**不报错**，于是"已固定形状"退化成一个没人检查的声明。
    #[test]
    fn dimension_overrides_use_the_symbols_from_the_probe() {
        let path = fixture("table_ok.onnx");
        let session = OrtSession::open_unchecked(&path, &rt(), &[]).expect("fixture must load");
        let probe = session.probe_io().expect("fixture must probe");

        let overrides = TableStructureConfig::default()
            .dimension_overrides(&probe, &path)
            .expect("the fixture input is dynamic and symbolically named");

        assert_eq!(
            overrides,
            vec![
                DimensionOverride::new("H", TABLE_STRUCTURE_SIDE_LEN as i64),
                DimensionOverride::new("W", TABLE_STRUCTURE_SIDE_LEN as i64),
            ],
            "H/W are input dims {TABLE_STRUCTURE_HEIGHT_DIM}/{TABLE_STRUCTURE_WIDTH_DIM}"
        );
    }

    /// 固定必须**从会话本身**观察到，而不是从"我们设了 override"推断出来。
    ///
    /// `N` 必须保持动态：`L`（第二维）只出现在输出上，输入侧不得被连带固定。
    #[test]
    fn fixing_the_shape_is_observable_on_the_opened_session() {
        let path = fixture("table_ok.onnx");
        let session =
            TableStructureSession::open(&cred(&path), &rt(), &TableStructureConfig::default())
                .expect("fixed session must open");

        assert_eq!(
            session.input_dims(),
            [
                -1,
                3,
                TABLE_STRUCTURE_SIDE_LEN as i64,
                TABLE_STRUCTURE_SIDE_LEN as i64
            ],
            "H/W must read back as fixed, and N must stay dynamic"
        );
        assert_eq!(session.contract().input_name, "x");
        assert!(session.config().fix_input_shape);
    }

    /// 关掉开关时必须**保持原样**：输入仍然是全动态的。
    ///
    /// 这条是"固定确实生效"的对照组 —— 没有它，`fixing_the_shape_is_observable...`
    /// 无法排除"这个模型本来就报 488"。
    #[test]
    fn disabling_the_switch_leaves_the_input_dynamic() {
        let path = fixture("table_ok.onnx");
        let config = TableStructureConfig {
            fix_input_shape: false,
            ..TableStructureConfig::default()
        };
        let session = TableStructureSession::open(&cred(&path), &rt(), &config)
            .expect("unfixed session must open");

        assert_eq!(session.input_dims(), [-1, 3, -1, -1]);
        assert_eq!(session.config(), &config);
    }

    // ------------------------------------------------------- 加载期完整性校验

    /// 一个独立的模型目录：登记查找是**按目录**的，共用 `tests/fixtures/table-onnx`
    /// 会让这些测试与那里的其他 fixture 互相看见对方的清单。
    struct ScratchModelDir {
        dir: PathBuf,
    }

    impl ScratchModelDir {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "rapid-ocr-table-load-{label}-{}-{id}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir should be creatable");
            Self { dir }
        }

        /// 把 `table_ok.onnx` 拷进来，返回新路径（basename 保持不变，登记按 basename 查）。
        fn with_ok_model(&self) -> PathBuf {
            let target = self.dir.join("table_ok.onnx");
            std::fs::copy(fixture("table_ok.onnx"), &target).expect("fixture should copy");
            target
        }

        fn register(&self, sha256: &str) {
            let manifest = format!(
                r#"{{
  "schema_version": 1,
  "id": "scratch",
  "family": "SLANet",
  "version": "v2.0.0",
  "files": [
    {{ "name": "table_ok.onnx", "role": "table_structure", "sha256": "{sha256}" }}
  ]
}}"#
            );
            std::fs::write(self.dir.join("manifest.json"), manifest)
                .expect("manifest should be writable");
        }
    }

    impl Drop for ScratchModelDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// §14.2② 的核心断言：**加载路径**自己校验，不依赖下载期或 `/api/models` 的显示。
    ///
    /// 同一个路径上内容被换掉，是下载期校验看不见的情形；只有加载期校验能拦住它。
    ///
    /// 审核点 2 之后拒绝发生在**凭证构造点**（registration 层，`open` 只接收凭证）：拿不到
    /// 凭证就到不了 ORT，"先探针再开会话"的调用方也无法在校验之前创建会话。
    #[test]
    fn a_model_whose_registered_hash_does_not_match_never_reaches_ort() {
        let scratch = ScratchModelDir::new("tampered");
        let path = scratch.with_ok_model();
        // 真哈希不是全零；登记一个全零哈希等价于"文件被换过了"。
        scratch.register(&"0".repeat(64));

        let error = crate::table::registration::verified_registered_model(&path, None)
            .expect_err("a mismatching hash must be refused at credential construction");
        assert!(
            matches!(error, RapidOcrError::HashMismatch { .. }),
            "the refusal must be locatable, got {error:?}"
        );

        // 探针是另一条加载入口，必须同样被拦住 —— 它自己构造凭证，摘要不符时同样到不了 ORT。
        let error = TableStructureModelInfo::probe(&path, &rt())
            .expect_err("the probe must be refused as well");
        assert!(
            matches!(error, RapidOcrError::HashMismatch { .. }),
            "{error:?}"
        );
    }

    /// 对照组：登记正确时加载必须**成功**，否则上面那条测试无法排除
    /// "这个路径本来就打不开"。
    #[test]
    fn a_model_registered_with_its_real_hash_loads() {
        let scratch = ScratchModelDir::new("matching");
        let path = scratch.with_ok_model();
        let real = crate::model_verify::sha256_file_cached(&path).expect("hash should compute");
        scratch.register(&real);

        let credential = crate::table::registration::verified_registered_model(&path, None)
            .expect("the registered hash must be accepted");
        assert_eq!(
            credential.actual_sha256(),
            Some(real.as_str()),
            "the credential must record the digest it verified"
        );
        let session =
            TableStructureSession::open(&credential, &rt(), &TableStructureConfig::default())
                .expect("a correctly registered model must load");
        assert_eq!(session.contract().input_name, "x");
    }

    /// 显式期望优先于登记：摘要在**凭证构造点**解析（`verified_registered_model`），
    /// 清单只是默认来源，显式参数是**部署**的决定（审核点 2：与文本、公式同一契约）。
    #[test]
    fn an_explicit_expected_hash_overrides_the_registration() {
        let scratch = ScratchModelDir::new("explicit");
        let path = scratch.with_ok_model();
        scratch.register(&"0".repeat(64));
        let real = crate::model_verify::sha256_file_cached(&path).expect("hash should compute");

        let credential = crate::table::registration::verified_registered_model(&path, Some(&real))
            .expect(
                "the explicit hash is the right one, so the wrong registration must not matter",
            );
        TableStructureSession::open(&credential, &rt(), &TableStructureConfig::default())
            .expect("the credential built from the explicit hash must load");
    }

    /// 没有清单时登记查找退化为“无期望凭证”（存在性照查、内容照绑定），而不是
    /// "跳过校验"：这是手工放置模型的路径，也是全部合成 fixture 走的路径。
    #[test]
    fn a_model_without_a_manifest_is_loaded_without_a_hash() {
        let scratch = ScratchModelDir::new("no-manifest");
        let path = scratch.with_ok_model();

        let credential = crate::table::registration::verified_registered_model(&path, None)
            .expect("a model with no registration has nothing to verify against");
        assert!(
            !credential.is_verified(),
            "no manifest must mean no verified digest"
        );
        TableStructureSession::open(&credential, &rt(), &TableStructureConfig::default())
            .expect("the credential must still bind the content it read");
    }

    /// 审核点 2 要求的表格级替换回归（与文本、公式域同一形状）：凭证在 `open` 这一层
    /// 消费的就是**校验那一刻的内容** —— 构造凭证之后把路径上的文件换成非 ONNX 字节，
    /// `TableStructureSession::open` 必须仍然成功，而此刻按路径重新构造凭证再开会失败。
    #[test]
    fn table_session_opens_the_bytes_the_credential_verified() {
        let scratch = ScratchModelDir::new("replaced-at-open");
        let path = scratch.with_ok_model();
        let credential = cred(&path);

        std::fs::write(&path, b"not an onnx model any more").expect("replace after credential");

        let session =
            TableStructureSession::open(&credential, &rt(), &TableStructureConfig::default())
                .expect("the bound content must open even though the path was replaced");
        assert_eq!(session.contract().input_name, "x");

        // 反面对照：此刻**重新**从路径构造凭证，绑定的是替换后的新字节，开卷必然失败 ——
        // 上面那次成功只能来自凭证里的那份快照。
        let stale = cred(&path);
        let failing = TableStructureSession::open(&stale, &rt(), &TableStructureConfig::default())
            .expect_err("the replaced content must not load");
        assert!(matches!(failing, RapidOcrError::Decode(_)), "{failing}");
    }

    /// 审核点 1 的端到端回归：凭证把**校验那一刻的内容**交给 ORT，而不是重新开卷路径。
    ///
    /// 构造凭证之后把文件替换成非 ONNX 字节：从凭证打开的会话必须仍然成功（内容是
    /// 校验时那份），而同一时刻按路径再开必须失败。两者合起来证明“ORT 不可能载入
    /// 校验之外的内容”不依赖校验与打开之间路径的稳定性。
    #[test]
    fn open_verified_loads_the_bound_bytes_and_not_the_replaced_path() {
        let scratch = ScratchModelDir::new("replaced-after-verify");
        let path = scratch.with_ok_model();
        let credential =
            crate::model_verify::VerifiedModel::verify(&path, None).expect("credential");

        std::fs::write(&path, b"not an onnx model any more").expect("replace after verification");

        let session = OrtSession::open_verified(&credential, &rt(), &[])
            .expect("the verified bytes must load regardless of what the path holds now");
        let io = session.probe_io().expect("probe");
        validate_table_structure_contract(&io, &path)
            .expect("the contract of the *verified* content still holds");

        let error = OrtSession::open_unchecked(&path, &rt(), &[])
            .expect_err("the replaced content must not load through a path-based open");
        assert!(matches!(error, RapidOcrError::Decode(_)), "{error}");
    }

    /// ORT 对**无法解析**的 override 名字不报错，只是不生效。
    ///
    /// 这条测试是 `require_applied` 存在的全部理由：没有它，"我们固定了形状"无法被证伪，
    /// 而性能差异会被归因到别的原因上。
    #[test]
    fn an_unknown_override_name_is_silently_ignored_by_ort() {
        let path = fixture("table_ok.onnx");
        let bogus = [DimensionOverride::new("NotADimensionInThisGraph", 488)];
        let session = OrtSession::open_unchecked(&path, &rt(), &bogus)
            .expect("ORT must not reject an unresolvable override name");
        let probe = session.probe_io().expect("fixture must probe");

        assert_eq!(
            probe.inputs[0].dims,
            [-1, 3, -1, -1],
            "an unresolvable name must have changed nothing"
        );

        let error = TableStructureConfig::default()
            .require_applied(&probe, &path)
            .expect_err("the post-condition must catch what ORT let through");
        let message = error.to_string();
        assert!(message.contains("did not take effect"), "{message}");
        assert!(message.contains("488"), "{message}");
    }

    /// 输入 H/W 已经是**别的**定长值时不能假装能固定它。
    ///
    /// 这类模型的形状在导出时就定了，`AddFreeDimensionOverrideByName` 改不了它；静默放行会让
    /// `input_side_len` 变成一个与运行事实无关的配置项。
    #[test]
    fn an_already_fixed_dimension_cannot_be_overridden() {
        let probe = ModelIoProbe {
            inputs: vec![TensorSpec::new(
                "x",
                vec![-1, 3, 256, 256],
                TensorElementType::Float32,
            )],
            outputs: Vec::new(),
        };

        let error = TableStructureConfig::default()
            .dimension_overrides(&probe, Path::new("fixed_256.onnx"))
            .expect_err("a fixed dimension is not overridable");
        let message = error.to_string();
        assert!(message.contains("fixed to 256"), "{message}");
        assert!(message.contains("cannot be overridden"), "{message}");
        assert!(message.contains("fixed_256.onnx"), "{message}");
    }

    /// 动态但**没有符号名**的维度同样无法固定 —— ORT 只能按符号名寻址。
    #[test]
    fn a_dynamic_dimension_without_a_symbol_cannot_be_overridden() {
        let probe = ModelIoProbe {
            inputs: vec![TensorSpec::new(
                "x",
                vec![-1, 3, -1, -1],
                TensorElementType::Float32,
            )],
            outputs: Vec::new(),
        };

        let error = TableStructureConfig::default()
            .dimension_overrides(&probe, Path::new("unnamed.onnx"))
            .expect_err("a dynamic dimension without a symbol cannot be addressed");
        let message = error.to_string();
        assert!(message.contains("dim_param"), "{message}");
        assert!(message.contains("symbol"), "{message}");
        assert!(message.contains("unnamed.onnx"), "{message}");
    }

    /// 尺寸已经等于 `input_side_len` 时**不施加**冗余 override。
    #[test]
    fn an_already_matching_size_emits_no_override() {
        let probe = ModelIoProbe {
            inputs: vec![TensorSpec::new(
                "x",
                vec![-1, 3, 488, 488],
                TensorElementType::Float32,
            )],
            outputs: Vec::new(),
        };

        let overrides = TableStructureConfig::default()
            .dimension_overrides(&probe, Path::new("fixed_488.onnx"))
            .expect("488 already matches, so nothing needs overriding");

        assert!(overrides.is_empty(), "got {overrides:?}");
    }

    /// `run_with` 必须按**契约名字**取回两路输出，且秩是 typed 的。
    ///
    /// 按名字而不是按下标：`output_names` 是模型声明顺序，把它当契约顺序，会让一次
    /// 两路输出换序的重新导出让框与结构概率被静默对调。
    #[test]
    fn run_with_returns_typed_views_in_contract_order() {
        let path = fixture("table_ok.onnx");
        let mut session =
            TableStructureSession::open(&cred(&path), &rt(), &TableStructureConfig::default())
                .expect("session must open");
        let input = ndarray::Array4::<f32>::zeros((2, 3, 488, 488));

        let (boxes_shape, tokens_shape, samples) = session
            .run_with(input.view(), |outputs| {
                Ok((
                    outputs.boxes.shape().to_vec(),
                    outputs.tokens.shape().to_vec(),
                    outputs.boxes.iter().take(1).copied().collect::<Vec<_>>(),
                ))
            })
            .expect("two-output run must succeed");

        assert_eq!(
            boxes_shape,
            vec![2, 4, TABLE_STRUCTURE_BOX_LAST_DIM as usize]
        );
        assert_eq!(
            tokens_shape,
            vec![2, 4, TABLE_STRUCTURE_TOKEN_LAST_DIM as usize]
        );
        assert_eq!(samples.len(), 1, "the declared views must be readable");
    }

    /// `open` 必须自己跑契约校验，而不是假定调用方已经探过一次。
    ///
    /// 存在性检查现在住在凭证构造点（`VerifiedModel::verify`）：文件不在时拿不到凭证，
    /// 错误类型与地点都必须可定位。
    #[test]
    fn session_open_validates_the_contract() {
        let renamed = TableStructureSession::open(
            &cred(&fixture("table_renamed_output.onnx")),
            &rt(),
            &TableStructureConfig::default(),
        )
        .expect_err("renamed outputs must be rejected before any inference");
        assert!(renamed.to_string().contains("output mismatch"), "{renamed}");

        let missing =
            crate::model_verify::VerifiedModel::verify(&fixture("does_not_exist.onnx"), None)
                .expect_err("a missing model must be located at credential construction");
        assert!(
            matches!(missing, RapidOcrError::FileNotFound(_)),
            "{missing}"
        );
    }
}

#[cfg(test)]
mod real_model_tests {
    use std::path::PathBuf;

    use crate::{
        config::RuntimeConfig,
        table::structure::{
            TABLE_STRUCTURE_OUTPUTS, TableStructureConfig, TableStructureModelInfo,
            TableStructureSession,
        },
    };

    /// 真实模型不在仓库内；缺失时必须 skip，不允许 panic 或回落到开发机路径。
    fn real_model() -> Option<PathBuf> {
        crate::test_support::table_structure_model_path()
    }

    /// 结构输入的正方形边长。
    const SIDE: usize = 488;

    /// 真实模型的 golden：两个输出名、dtype、末维。
    ///
    /// 这是 `docs/09-table-recognition-execution.md` §3.2 的最后一条验收项。
    #[test]
    fn real_model_matches_the_output_golden() {
        let Some(path) = real_model() else {
            return;
        };
        let info = TableStructureModelInfo::probe(&path, &RuntimeConfig::default())
            .expect("real model must satisfy the contract");
        assert_eq!(info.box_output_name, TABLE_STRUCTURE_OUTPUTS[0]);
        assert_eq!(info.structure_output_name, TABLE_STRUCTURE_OUTPUTS[1]);
    }

    /// 出厂清单必须真的登记了出厂模型。
    ///
    /// 这条不能省：`verified_registered_model` 在没有清单时退化为“无期望凭证”，因此
    /// "加载路径会校验"与"这个模型**被**校验了"是两句不同的话 —— 只有这条测试把
    /// 后者钉住。它同时是 `models/table/manifest.json` 里那个哈希的回归守卫。
    #[test]
    fn real_model_is_registered_in_the_shipped_manifest() {
        let Some(path) = real_model() else {
            return;
        };
        let registration = crate::table::registration::registration_for(&path)
            .expect("the shipped manifest must parse")
            .expect(
                "the shipped model must be registered next to itself; without it the load path \
                 verifies nothing",
            );
        assert_eq!(registration.name, "slanet-plus.onnx");
        assert_eq!(registration.size_bytes, Some(7_758_305));
        assert_eq!(
            crate::model_verify::sha256_file_cached(&path).expect("hash should compute"),
            registration.sha256,
            "the shipped manifest must record the hash of the shipped model"
        );
        // 有效期望哈希与登记一致时，加载期校验必须**真的跑过**并留下已校验凭证。
        let credential = crate::table::registration::verified_registered_model(&path, None)
            .expect("the registered hash must match the file");
        assert_eq!(
            credential.actual_sha256(),
            Some(registration.sha256.as_str())
        );
        assert!(
            credential.is_verified(),
            "the shipped manifest must carry a real expectation"
        );
    }

    /// 真实模型的 `character` metadata 是换行分隔的 48 个 token，且宽度与末维一致（48+2=50）。
    #[test]
    fn real_model_vocabulary_matches_the_graph_width() {
        let Some(path) = real_model() else {
            return;
        };
        let info = TableStructureModelInfo::probe(&path, &RuntimeConfig::default())
            .expect("real model must satisfy the contract");
        assert_eq!(
            info.character_tokens.len() + 2,
            info.structure_last_dim,
            "vocabulary width and graph width must agree"
        );
        assert_eq!(info.character_tokens.len(), 48);
    }

    // ---------------------------------------------------------------- 形状固定

    /// 确定性伪随机输入：LCG 整数模式转 f32，不引入 `rand` 依赖。
    ///
    /// 用伪随机而不是全零/全一：常数输入会让很多分歧（例如两路输出被对调、
    /// 某个分支只在非常数输入下走到）不被观察到。
    fn deterministic_input() -> ndarray::Array4<f32> {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut values = Vec::with_capacity(3 * SIDE * SIDE);
        for _ in 0..(3 * SIDE * SIDE) {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let bucket = (state >> 33) as u32 % 4096;
            values.push((bucket as f32 / 2048.0) - 1.0);
        }
        ndarray::Array4::from_shape_vec((1, 3, SIDE, SIDE), values).expect("shape matches")
    }

    /// 一次推理的两路输出。数值取 `to_bits`：视图不能逃逸回调，
    /// 而位模式比较使"逐位一致"成为可断言的事实而不是"看起来相近"。
    struct Inference {
        boxes_shape: Vec<usize>,
        tokens_shape: Vec<usize>,
        boxes_bits: Vec<u32>,
        tokens_bits: Vec<u32>,
    }

    fn run_once(session: &mut TableStructureSession, input: &ndarray::Array4<f32>) -> Inference {
        session
            .run_with(input.view(), |outputs| {
                Ok(Inference {
                    boxes_shape: outputs.boxes.shape().to_vec(),
                    tokens_shape: outputs.tokens.shape().to_vec(),
                    boxes_bits: outputs.boxes.iter().map(|value| value.to_bits()).collect(),
                    tokens_bits: outputs.tokens.iter().map(|value| value.to_bits()).collect(),
                })
            })
            .expect("inference must succeed")
    }

    fn open(session_path: &std::path::Path, config: TableStructureConfig) -> TableStructureSession {
        // 与探针同样走 registration 解析期望（审核点 2）：真实模型旁的清单会在这里生效。
        let credential = crate::table::registration::verified_registered_model(session_path, None)
            .expect("credential must build");
        TableStructureSession::open(&credential, &RuntimeConfig::default(), &config)
            .expect("session must open")
    }

    /// `docs/09-table-recognition-execution.md` §6.2 第 1 条：固定 H/W 前后，
    /// **同一输入下两路输出逐位一致**。
    ///
    /// 形状固定是编译期策略，一旦它改变了数值语义，这里会以位级差异暴露出来，
    /// 而不是以"输出看起来差不多"糊过去。同时锁住"`L` 没被连带固定"：
    /// 两路输出的第二维（`L`）必须相同且随输入变化，而不是被钉死在 `Loop` 的 500 上限。
    #[test]
    fn real_model_shape_fixing_is_bit_exact() {
        let Some(path) = real_model() else {
            return;
        };
        let input = deterministic_input();
        let mut fixed = open(&path, TableStructureConfig::default());
        let mut unfixed = open(
            &path,
            TableStructureConfig {
                fix_input_shape: false,
                ..TableStructureConfig::default()
            },
        );

        assert_eq!(fixed.input_dims(), [-1, 3, SIDE as i64, SIDE as i64]);
        assert_eq!(
            unfixed.input_dims(),
            [-1, 3, -1, -1],
            "the control group must really be dynamic"
        );

        let observed_fixed = run_once(&mut fixed, &input);
        let observed_unfixed = run_once(&mut unfixed, &input);

        assert_eq!(
            observed_fixed.boxes_shape, observed_unfixed.boxes_shape,
            "fixing the input shape must not change the box output shape"
        );
        assert_eq!(
            observed_fixed.tokens_shape, observed_unfixed.tokens_shape,
            "fixing the input shape must not change the token output shape"
        );
        assert_eq!(observed_fixed.boxes_shape.len(), 3);
        assert_eq!(observed_fixed.boxes_shape[2], 8);
        assert_eq!(observed_fixed.tokens_shape[2], 50);
        assert!(
            observed_fixed.boxes_shape[1] < 500,
            "`L` must be produced by the model's early exit, not by the Loop trip limit: got {}",
            observed_fixed.boxes_shape[1]
        );

        assert_eq!(
            observed_fixed.boxes_bits, observed_unfixed.boxes_bits,
            "box values must be bit-identical"
        );
        assert_eq!(
            observed_fixed.tokens_bits, observed_unfixed.tokens_bits,
            "token values must be bit-identical"
        );
    }

    /// `docs/09-table-recognition-execution.md` §6.2 第 3 条：记录固定前后
    /// `table_structure_infer_ms` 的**数值**。
    ///
    /// **刻意不断言性能**：本阶段的产出是"数字已记录"，不是"固定形状更快"。
    /// 数字用 `cargo test --lib table::structure::real_model_tests -- --nocapture` 查看。
    #[test]
    fn real_model_records_inference_time_before_and_after_fixing() {
        let Some(path) = real_model() else {
            return;
        };
        let input = deterministic_input();
        let configs = [
            ("fixed", TableStructureConfig::default()),
            (
                "unfixed",
                TableStructureConfig {
                    fix_input_shape: false,
                    ..TableStructureConfig::default()
                },
            ),
        ];

        for (label, config) in configs {
            let mut session = open(&path, config);
            // 预热：第一次运行包含算子选择与激活缓冲分配，不进入样本。
            run_once(&mut session, &input);

            let mut samples = Vec::with_capacity(15);
            for _ in 0..15 {
                let started = std::time::Instant::now();
                run_once(&mut session, &input);
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }

            let stats = crate::evaluation::stats::Stats::from_samples(samples);
            assert_eq!(stats.samples, 15, "every sample must be recorded");
            println!(
                "table_structure_infer_ms[{label}] p50={:.3} mean={:.3} p95={:.3}",
                stats.p50_ms, stats.mean_ms, stats.p95_ms
            );
        }
    }
}

/// E12 对 `docs/09-table-recognition-execution.md` §13.4 那条**开放问题**的实验。
///
/// # 问题
///
/// §13.4 登记：「SLANet 含 `Loop` 控制流算子，DirectML 对控制流子图的支持**未验证**」。
/// E5 与 E11 都把这条留在"未覆盖风险"里。它不是可以靠读文档回答的问题 —— ORT 的 EP
/// 是按**子图**分派的，控制流算子（`Loop`）的执行语义由框架而非 EP 负责，所以
/// "DML 支不支持"这句话对 `Loop` 本身没有确定的答案，只有跑一次才知道。
///
/// # 为什么这个实验在本机**可以**做（与 §13.4 原稿相反）
///
/// §13.4 写的阻塞是"本机无 DirectML DLL"，依据是 **Python** 的 `onnxruntime` 只列出
/// `AzureExecutionProvider` + `CPUExecutionProvider`。那个依据对本仓库不成立：仓库的
/// Rust 构建静态链接了自己的 ORT 1.28 并加载 `DirectML.dll`（E5 的 provider smoke 实测
/// `direct_ml` 变体 `selected_ep == direct_ml`、`fallback_to_cpu false`）。所以这条阻塞
/// 是**工具链错配**，不是硬件缺失。
///
/// # 为什么必须是 `#[cfg(feature = "directml-provider")]`
///
/// 默认构建里 DirectML 根本不在 EP 链上（`src/runtime/provider.rs` 的 `feature_disabled`），
/// 拿默认构建跑这个实验只会得到一条"provider 不可用"的错误，那**证明不了**关于 `Loop` 的
/// 任何事。跑法：
///
/// ```text
/// cargo test --features directml-provider --lib \
///     table::structure::direct_ml_experiment -- --nocapture
/// ```
///
/// 结果无论正负都要登记（§13.4 与 `docs/07-table-recognition.md` §14）：
/// 成功 → DirectML 路径对表格模型可用，"控制流不受支持"可以关闭；
/// 失败 → 这就是一条**实测**出来的限制，而不是一条抄来的说法。
#[cfg(feature = "directml-provider")]
#[cfg(test)]
mod direct_ml_experiment {
    use crate::{
        config::{ProviderPreference, RuntimeConfig},
        table::structure::{TableStructureConfig, TableStructureSession},
    };

    fn direct_ml_runtime() -> RuntimeConfig {
        RuntimeConfig {
            provider_preference: ProviderPreference::DirectMl { device_id: 0 },
            // 关键：让"拿不到 DirectML"变成一条错误，而不是静默回落到 CPU。
            // 没有这一条，下面无论断言什么都可能是在 CPU 上通过的。
            fail_if_provider_unavailable: true,
            ..RuntimeConfig::default()
        }
    }

    /// SLANet-plus 在 DirectML 上能否**创建会话并完成一次推理**。
    ///
    /// 这个测试**不断言成功**。它的产出是"结论 + 打印出来的证据"：在无法保证 DML 可用的
    /// 机器上，把"不支持"断言成失败会让测试在别人的机器上无意义地红。真正要守住的是
    /// **不要静默回落到 CPU** —— 那一项由 `fail_if_provider_unavailable: true` 保证，
    /// 而它一旦失效（provider 被静默换掉），下面的 `selected_ep` 断言会失败。
    #[test]
    fn the_real_table_model_runs_on_direct_ml_or_says_why_not() {
        let Some(path) = crate::test_support::table_structure_model_path() else {
            return;
        };
        let runtime = direct_ml_runtime();
        let credential = crate::table::registration::verified_registered_model(&path, None)
            .expect("the real model must pass load-time verification");

        let opened =
            TableStructureSession::open(&credential, &runtime, &TableStructureConfig::default());
        let mut session = match opened {
            Ok(session) => session,
            Err(error) => {
                println!("DIRECT_ML_VERDICT=session_open_failed");
                println!("DIRECT_ML_ERROR={error}");
                return;
            }
        };

        // 走到了这里，说明 provider 确实被接受了（`fail_if_provider_unavailable` 没触发）。
        let resolution = session.provider_resolution();
        println!("DIRECT_ML_VERDICT=session_opened");
        println!("DIRECT_ML_SELECTED_EP={:?}", resolution.selected_ep);
        println!("DIRECT_ML_FALLBACK={}", resolution.fallback_used);
        println!("DIRECT_ML_OPSET={:?}", session.onnx_opset());
        assert_eq!(
            resolution.selected_ep,
            crate::runtime::provider::ResolvedExecutionProvider::DirectMl,
            "the DirectML chain must be the one actually handed to ORT"
        );
        assert!(
            !resolution.fallback_used,
            "a fallback would mean the timing and the control-flow answer both describe CPU"
        );

        // 真正的实验：跑一次。`Loop` 的控制流在这里被求值，任何算子级不支持都会在这里
        // 变成一条错误（而不是在创建期）。
        let side = TableStructureConfig::default().input_side_len;
        let input = ndarray::Array4::<f32>::from_elem((1, 3, side, side), 0.25);
        let outcome = session.run_with(input.view(), |outputs| {
            Ok((
                outputs.boxes.shape().to_vec(),
                outputs.tokens.shape().to_vec(),
            ))
        });

        match outcome {
            Ok((boxes, tokens)) => {
                println!("DIRECT_ML_VERDICT=inference_succeeded");
                println!("DIRECT_ML_BOXES_SHAPE={boxes:?}");
                println!("DIRECT_ML_TOKENS_SHAPE={tokens:?}");
                // `Loop` 的 trip 次数依 `eos` 早退，所以 L 必须远小于 500 的硬上限。
                // 若 DML 把控制流"跑满"，L 会顶到上限 —— 这是它最可能出的错法。
                assert_eq!(boxes.first().copied(), Some(1), "batch must be preserved");
                if let Some(&steps) = tokens.get(1) {
                    assert!(
                        steps < 500,
                        "the decoder must early-exit on `eos`; {steps} steps means the \
                         control flow ran to the trip cap"
                    );
                }
            }
            Err(error) => {
                println!("DIRECT_ML_VERDICT=inference_failed");
                println!("DIRECT_ML_ERROR={error}");
                let text = error.to_string();
                // 只打印不判定的测试无法把"结论"与"任何一条错误"区分开。这里钉住**两条**
                // 可伪证的性质，让这条记录会随现实变化而失败：
                //
                // 1. 失败必须来自 DML 的执行提供者本身。缺 DLL、驱动异常、显存不足同样会
                //    产生错误，但那些错误说明这次测量根本没测到"DML 对这个图的支持"，
                //    结论就不成立。`DmlExecutionProvider` 只出现在 EP 自己的实现里。
                assert!(
                    text.contains("DmlExecutionProvider"),
                    "the failure must come from the DirectML execution provider itself, \
                     otherwise this run measured the environment rather than the graph: {text}"
                );
                // 2. 失败必须落在控制流上 —— 这是 `docs/07-table-recognition.md` §12 R7 与
                //    `docs/09-table-recognition-execution.md` §13.5.2 记录的根因（`Loop` 体内
                //    的 `Cast`）。若将来失败点变成别的算子，说明文档记的根因过时了，
                //    必须同步修改文档，而不是让这条断言静默通过。
                assert!(
                    text.contains("Loop"),
                    "the documented root cause is the `Loop` control flow; a different \
                     failure means the docs must be revisited: {text}"
                );
            }
        }
    }
}
