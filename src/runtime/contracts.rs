//! 共享 ONNX IO 契约验证工具。
//!
//! 该模块只描述 tensor 名称、rank、维度、符号名与元素类型，不绑定普通 OCR 或公式领域。
//! 领域 session 使用这些工具构造自己的 typed contract。

use std::path::Path;

use ort::value::TensorElementType;

use crate::error::{RapidOcrError, Result};

/// ONNX 用来表示"该维度是动态的"的哨兵值。
///
/// `dim_value` 缺失（只有 `dim_param`，或两者都缺）时 ONNX Runtime 报 `-1`。
pub const DYNAMIC_DIM: i64 = -1;

/// 通用 ONNX tensor 签名描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorSpec {
    pub name: String,
    pub rank: usize,
    /// 每个维度的定长值；动态维度为 `DYNAMIC_DIM`。
    pub dims: Vec<i64>,
    /// 每个维度的 `dim_param`（符号）名，长度等于 [`Self::rank`]；定长或未命名的维度为 `None`。
    ///
    /// 为什么必须保留：ONNX Runtime 的 free-dimension override
    /// （`AddFreeDimensionOverrideByName`）**只能按符号名寻址**，而"覆盖哪个维度"不能靠
    /// 硬编码 —— SLANet 的符号名是 `paddle2onnx` 转换时按出现顺序生成的
    /// `DynamicDimension.1` 这类无语义位置名，重转一次就会变。符号名只能从模型自己读出来，
    /// 探针丢掉它，调用方就只能猜。
    pub dimension_symbols: Vec<Option<String>>,
    pub element_type: TensorElementType,
}

impl TensorSpec {
    /// 构造一个**没有符号名**的签名，供测试与合成 fixture 使用。
    ///
    /// 只有当 `dims` 里没有 `DYNAMIC_DIM` 时它才是完整描述：动态维度若没有符号名，
    /// `require_dynamic_dim_symbol` 会拒绝，因为那种维度无法被 override 寻址。
    pub fn new(name: impl Into<String>, dims: Vec<i64>, element_type: TensorElementType) -> Self {
        let rank = dims.len();
        Self {
            name: name.into(),
            rank,
            dims,
            dimension_symbols: vec![None; rank],
            element_type,
        }
    }
}

/// 对已加载 session 进行通用 IO 探测的结果。
#[derive(Debug, Clone)]
pub struct ModelIoProbe {
    pub inputs: Vec<TensorSpec>,
    pub outputs: Vec<TensorSpec>,
}

pub fn require_single_input<'a>(
    probe: &'a ModelIoProbe,
    model_path: &Path,
    model_kind: &str,
) -> Result<&'a TensorSpec> {
    if probe.inputs.len() != 1 {
        return Err(RapidOcrError::Config(format!(
            "{model_kind} model must expose exactly one input, got {} (model={})",
            probe.inputs.len(),
            model_path.display()
        )));
    }
    Ok(&probe.inputs[0])
}

pub fn require_single_output<'a>(
    probe: &'a ModelIoProbe,
    model_path: &Path,
    model_kind: &str,
) -> Result<&'a TensorSpec> {
    if probe.outputs.len() != 1 {
        return Err(RapidOcrError::Config(format!(
            "{model_kind} model must expose exactly one output, got {} (model={})",
            probe.outputs.len(),
            model_path.display()
        )));
    }
    Ok(&probe.outputs[0])
}

/// 要求模型声明的输出**恰好**是 `expected` 列出的这一组，并按 `expected` 的顺序把它们返回。
///
/// 存在的理由：`require_single_output` 对多输出模型直接报错，而
/// [`crate::runtime::session::OrtSession::run_arrayd_view_with`]"只取第一个输出"对多输出模型
/// 是**静默**错误。SLANet 表格结构模型正好有**两路**输出
/// （`save_infer_model/scale_0.tmp_0` 单元格框 + `save_infer_model/scale_1.tmp_0` 结构 token 概率），
/// 少了任何一路都不能算通过契约 —— 所以这里要求精确匹配，既不接受缺项，也不接受未声明的多余输出。
///
/// 返回值顺序等于 `expected` 顺序（不是模型声明顺序），调用方因此可以用固定下标访问，
/// 而不必再按名字查找一遍。
pub fn require_outputs<'a>(
    probe: &'a ModelIoProbe,
    model_path: &Path,
    model_kind: &str,
    expected: &[&str],
) -> Result<Vec<&'a TensorSpec>> {
    let actual = probe
        .outputs
        .iter()
        .map(|spec| spec.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let mut resolved = Vec::with_capacity(expected.len());
    for name in expected {
        match probe.outputs.iter().find(|spec| spec.name == *name) {
            Some(spec) => resolved.push(spec),
            None => {
                return Err(RapidOcrError::Config(format!(
                    "{model_kind} model output mismatch: expected [{}], got {} outputs [{}] (missing `{name}`) (model={})",
                    expected.join(", "),
                    probe.outputs.len(),
                    actual,
                    model_path.display()
                )));
            }
        }
    }

    let unexpected = probe
        .outputs
        .iter()
        .map(|spec| spec.name.as_str())
        .filter(|name| !expected.contains(name))
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(RapidOcrError::Config(format!(
            "{model_kind} model output mismatch: expected [{}], got {} outputs [{}] (unexpected [{}]) (model={})",
            expected.join(", "),
            probe.outputs.len(),
            actual,
            unexpected.join(", "),
            model_path.display()
        )));
    }

    Ok(resolved)
}

pub fn require_tensor(
    spec: &TensorSpec,
    io_kind: &str,
    expected_rank: usize,
    expected_type: TensorElementType,
    model_path: &Path,
) -> Result<()> {
    if spec.rank != expected_rank {
        return Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` rank mismatch: expected {expected_rank}, got {} (model={})",
            spec.name,
            spec.rank,
            model_path.display()
        )));
    }
    if spec.element_type != expected_type {
        return Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` dtype mismatch: expected {}, got {} (model={})",
            spec.name,
            element_type_label(expected_type),
            element_type_label(spec.element_type),
            model_path.display()
        )));
    }
    Ok(())
}

fn element_type_label(element_type: TensorElementType) -> String {
    match element_type {
        TensorElementType::Float32 => "FLOAT32".to_string(),
        TensorElementType::Float16 => "FLOAT16".to_string(),
        TensorElementType::Int64 => "INT64".to_string(),
        TensorElementType::Int32 => "INT32".to_string(),
        TensorElementType::Uint8 => "UINT8".to_string(),
        TensorElementType::Int8 => "INT8".to_string(),
        other => format!("{other:?}"),
    }
}

pub fn require_fixed_dim(
    spec: &TensorSpec,
    io_kind: &str,
    dim_index: usize,
    expected: i64,
    model_path: &Path,
) -> Result<()> {
    let Some(actual) = spec.dims.get(dim_index) else {
        return Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` is missing dim index {dim_index} (dims={:?}, model={})",
            spec.name,
            spec.dims,
            model_path.display()
        )));
    };
    if *actual != expected {
        return Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` dim mismatch at index {dim_index}: expected {expected}, got {actual} (dims={:?}, model={})",
            spec.name,
            spec.dims,
            model_path.display()
        )));
    }
    Ok(())
}

/// 取指定维度的 `dim_param` 符号名，供 free-dimension override 寻址。
///
/// 三种失败各有不同含义，所以不合并成一句：
///
/// - 维度不存在 → 模型 rank 与调用方假设不符；
/// - 维度已经是定长 → override 无从施加（该维度已被转换时固定住）；
/// - 维度是动态的但**没有符号名** → ONNX Runtime 无法按名寻址它，硬编码一个猜测的名字
///   会静默失效（override 不报错，只是不生效），必须显式失败。
pub fn require_dynamic_dim_symbol<'a>(
    spec: &'a TensorSpec,
    io_kind: &str,
    dim_index: usize,
    model_path: &Path,
) -> Result<&'a str> {
    let Some(actual) = spec.dims.get(dim_index) else {
        return Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` is missing dim index {dim_index} (dims={:?}, model={})",
            spec.name,
            spec.dims,
            model_path.display()
        )));
    };
    if *actual != DYNAMIC_DIM {
        return Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` dim {dim_index} is already fixed to {actual}, so it cannot be overridden (dims={:?}, model={})",
            spec.name,
            spec.dims,
            model_path.display()
        )));
    }
    match spec
        .dimension_symbols
        .get(dim_index)
        .and_then(Option::as_deref)
    {
        Some(symbol) => Ok(symbol),
        None => Err(RapidOcrError::Config(format!(
            "model {io_kind} `{}` dim {dim_index} is dynamic but carries no `dim_param` symbol, and ONNX Runtime can only override a dimension by symbol name (dims={:?}, symbols={:?}, model={})",
            spec.name,
            spec.dims,
            spec.dimension_symbols,
            model_path.display()
        ))),
    }
}
