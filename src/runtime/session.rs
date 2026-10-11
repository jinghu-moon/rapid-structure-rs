use std::path::Path;

use ndarray::{Array2, ArrayView2, ArrayView3, ArrayViewD, Ix2, Ix3, Ix4};
use ort::{
    inputs,
    session::{
        Session,
        builder::{GraphOptimizationLevel, SessionBuilder},
    },
    value::{TensorRef, ValueType},
};

use crate::{
    config::RuntimeConfig,
    error::{RapidOcrError, Result},
    runtime::contracts::{ModelIoProbe, TensorSpec},
    runtime::provider::{
        ProviderResolution, ResolvedExecutionProvider, resolve_execution_providers,
    },
};

/// DirectML 能接受的最高 ONNX opset。
///
/// 来源：DirectML 执行提供程序的官方约束（模型必须导出为 opset ≤ 20）。
/// ⚠️ **本机无法验证这条约束本身**（见 `docs/09-table-recognition-execution.md` §13.4）；
/// 这里用它做的检查是**把一条模糊的 ORT 失败变成一条能定位的错误**，不是新增一个能力。
pub const DIRECT_ML_MAX_OPSET: u32 = 20;

/// `selected_ep == DirectMl` 时的 opset 后置条件。
///
/// 为什么要有这一步：DirectML 对超范围 opset 的拒绝方式不是一句清楚的错误，而可能是
/// 逐节点回退到 CPU（于是"用了 DirectML"变成假声明）或创建期失败。把约束写在这里，
/// 失败点就从"某个算子"前移到"这个模型"。
///
/// `opset` 为 `None`（后端不支持 Model Editor API、读不到 opset）时**放行**：
/// 读不到不等于违反，此时报错会拦下本来能跑的模型。
pub fn require_direct_ml_opset(opset: Option<u32>, model_path: &Path) -> Result<()> {
    match opset {
        Some(opset) if opset > DIRECT_ML_MAX_OPSET => Err(RapidOcrError::Config(format!(
            "model {} declares ONNX opset {opset}, but the DirectML execution provider accepts at \
             most opset {DIRECT_ML_MAX_OPSET}; re-export the model or use the CPU/CUDA provider",
            model_path.display()
        ))),
        _ => Ok(()),
    }
}

/// 一次会话构造要施加的 ONNX free-dimension override。
///
/// `name` 必须是模型里的 **`dim_param` 符号名**（`AddFreeDimensionOverrideByName` 按符号
/// 寻址，不按 denotation；`SLANet-plus` 没有任何 denotation，所以
/// `with_dimension_override_by_denotation` 对它完全无效）。名字应当由
/// [`crate::runtime::contracts::require_dynamic_dim_symbol`] **从模型探针读出**，而不是写成常量。
///
/// 为什么 override 是**逐会话**的参数、而不是 `RuntimeConfig` 的字段：符号名
/// `DynamicDimension.<n>` 是 `paddle2onnx` 按维度出现顺序生成的位置名，普通 OCR 的模型里
/// 同样会出现同名符号。把它挂到全局 `RuntimeConfig` 上会让"给表格模型固定 H/W"这件事
/// 顺带改掉页级 det/rec 模型的维度 —— 那是静默的、跨领域的副作用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionOverride {
    pub name: String,
    pub size: i64,
}

impl DimensionOverride {
    pub fn new(name: impl Into<String>, size: i64) -> Self {
        Self {
            name: name.into(),
            size,
        }
    }
}

/// 由 selected EP 决定的**会话构建**选项。
///
/// # 为什么这不是"优化"，而是必要条件
///
/// DirectML 不支持 memory pattern 优化，也不支持 `ORT_PARALLEL` 执行模式。把它们留在
/// 默认值上不是"慢一点"，而是会话创建失败或行为未定义。官方文档同时要求 DirectML 的
/// 模型 opset ≤ 20（[`DIRECT_ML_MAX_OPSET`]）并偏好静态形状（见
/// [`crate::runtime::session::DimensionOverride`]）。
///
/// # 为什么 CPU / CUDA 一个 setter 都不调用
///
/// ORT 的默认值就是本结构体里的 [`Self::ORT_DEFAULTS`]（memory pattern 开、顺序执行）。
/// [`Self::apply`] 因此只在**偏离**默认值时才调用 setter —— CPU 与 CUDA 路径的会话构建
/// 与引入本策略之前**逐条调用相同**，于是"这次改动没有影响页级性能"不需要靠推测，
/// 而是由构造方式保证的。
///
/// CUDA 侧的 `arena_extend_strategy = kNextPowerOfTwo` 与
/// `cudnn_conv_algo_search = EXHAUSTIVE` **刻意不显式设置**：它们已经是 ORT 的默认值，
/// 显式写一遍只会新增一处会与上游漂移的常量，而不改变任何行为（奥卡姆剃刀）。
///
/// # 为什么需要 [`Self::must_be_explicit`]：`false` 不等于"无所谓"
///
/// 只按"是否偏离 ORT 默认值"来决定施加哪些 setter 会漏掉一个真实差别：
/// `ORT_SEQUENTIAL` **恰好就是** ORT 的默认执行模式，而它同时是 DirectML 的**必要条件**。
/// 如果 DirectML 因为"值等于默认值"而不下发这个 setter，那么 DirectML 的正确性就依赖
/// 上游默认值永远不变 —— 这正是本条策略要消除的那类隐式依赖。所以"恰好等于默认值"
/// 与"必须显式保证"被拆成两个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionBuildPolicy {
    /// 是否启用 ORT 的 memory pattern 优化（`false` → `DisableMemPattern`）。
    pub memory_pattern: bool,
    /// 是否启用 `ORT_PARALLEL` 执行模式（`false` → `ORT_SEQUENTIAL`）。
    pub parallel_execution: bool,
    /// 是否把上面两个值**显式**下发给 ORT，即使它们恰好等于 ORT 的默认值。
    ///
    /// `false` = "ORT 的默认值就是我们要的，不必多说"（CPU / CUDA，保持调用序列不变）；
    /// `true` = "这是必要条件，不能让上游默认值替我保证"（DirectML）。
    pub must_be_explicit: bool,
}

impl SessionBuildPolicy {
    /// ONNX Runtime 自己的默认值，也是 CPU / CUDA 采用的策略。
    pub const ORT_DEFAULTS: Self = Self {
        memory_pattern: true,
        parallel_execution: false,
        must_be_explicit: false,
    };

    /// 某个 selected EP 要求的构建选项。
    pub fn for_provider(selected_ep: ResolvedExecutionProvider) -> Self {
        match selected_ep {
            ResolvedExecutionProvider::DirectMl => Self {
                memory_pattern: false,
                parallel_execution: false,
                // 两项都必须显式下发，见 [`Self::must_be_explicit`]。
                must_be_explicit: true,
            },
            // CPU 与 CUDA 都保持 ORT 默认：CPU 的 `Level3` + memory arena 组合已实测最优
            // （`docs/07-table-recognition.md` §5.3），CUDA 的两个默认值就是官方推荐值。
            ResolvedExecutionProvider::Cpu | ResolvedExecutionProvider::Cuda => Self::ORT_DEFAULTS,
        }
    }

    /// 本策略需要**显式施加**的选项，按施加顺序。空列表 = 一个 setter 都不调用。
    ///
    /// 把"要设什么"做成一个可被断言的值，而不是埋在 `if` 里：这样"CPU 路径一条 builder 调用
    /// 都没多"是一句能被测试**证明**的话（`required_options().is_empty()`），而不是注释里的
    /// 承诺。
    pub fn required_options(&self) -> Vec<SessionBuildOption> {
        if !self.must_be_explicit {
            return Vec::new();
        }
        vec![
            SessionBuildOption::MemoryPattern(self.memory_pattern),
            SessionBuildOption::ParallelExecution(self.parallel_execution),
        ]
    }

    /// 把策略施加到 builder 上。**只调用 [`Self::required_options`] 里的那几个 setter**。
    pub fn apply(&self, builder: SessionBuilder) -> Result<SessionBuilder> {
        let mut builder = builder;
        for option in self.required_options() {
            builder = match option {
                SessionBuildOption::MemoryPattern(enable) => {
                    builder.with_memory_pattern(enable).map_err(ort_error)?
                }
                SessionBuildOption::ParallelExecution(enable) => {
                    builder.with_parallel_execution(enable).map_err(ort_error)?
                }
            };
        }
        Ok(builder)
    }
}

/// [`SessionBuildPolicy`] 里的一个 builder 选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBuildOption {
    /// `with_memory_pattern`：`true` → `EnableMemPattern`，`false` → `DisableMemPattern`。
    MemoryPattern(bool),
    /// `with_parallel_execution`：`true` → `ORT_PARALLEL`，`false` → `ORT_SEQUENTIAL`。
    ParallelExecution(bool),
}

#[derive(Debug)]
pub struct OrtSession {
    session: Session,
    model_path: String,
    provider_resolution: ProviderResolution,
    /// 打开会话时下发给 ONNX Runtime 的 `(intra, inter)` 线程数；`None` 表示**没有配置**
    /// 该线程数（没有调用对应的 `with_*_threads`），ONNX Runtime 保留自己的默认值，
    /// 它不表示 0 线程。
    ///
    /// 这里的值是 [`RuntimeConfig::effective_session_threads`] 的输出，也就是“这份配置要求
    /// ORT 用什么”，不是 ORT 内部最终解析出的线程数（那不可观测）。
    session_threads: (Option<usize>, Option<usize>),
    /// 模型的 ONNX 域 opset；见 [`OrtSession::onnx_opset`]。
    onnx_opset: Option<u32>,
    pub output_names: Vec<String>,
}

fn ort_error<E: std::fmt::Display>(error: E) -> RapidOcrError {
    RapidOcrError::Decode(format!("ONNX Runtime error: {error}"))
}

impl OrtSession {
    /// Opens an ONNX model without domain contract validation.
    ///
    /// 优化级别、线程、provider 与 `dimension_overrides` 都只在 [`open_session`] 里施加一次。
    /// Domain sessions must call [`Self::probe_io`] and validate their typed
    /// contract through [`crate::runtime::contracts`].
    pub fn open_unchecked(
        model_path: &Path,
        runtime_cfg: &RuntimeConfig,
        dimension_overrides: &[DimensionOverride],
    ) -> Result<Self> {
        let opened = open_session(model_path, runtime_cfg, dimension_overrides)?;
        let output_names = opened
            .session
            .outputs()
            .iter()
            .map(|value| value.name().to_string())
            .collect();
        Ok(Self {
            session: opened.session,
            model_path: model_path.display().to_string(),
            provider_resolution: opened.provider_resolution,
            session_threads: opened.session_threads,
            onnx_opset: opened.onnx_opset,
            output_names,
        })
    }

    /// 模型的 ONNX 域 opset（`ai.onnx`）；后端不支持 Model Editor API 时为 `None`。
    ///
    /// 它是 DirectML 的 opset 约束（[`DIRECT_ML_MAX_OPSET`]）唯一可观测的来源：
    /// 没有它，"这个模型能不能交给 DirectML"就只能等 ORT 在某个算子上失败。
    pub fn onnx_opset(&self) -> Option<u32> {
        self.onnx_opset
    }

    /// 打开会话时下发给 ONNX Runtime 的 `(intra, inter)` 线程数。
    ///
    /// 直接构造 `RuntimeConfig` 的调用方（`FormulaSession` / `formula_bench` /
    /// `formula_eval`）可以用它证明自己拿到的是与引擎路径同一套线程解析结果。
    pub fn session_threads(&self) -> (Option<usize>, Option<usize>) {
        self.session_threads
    }

    pub fn metadata_custom(&self, key: &str) -> Result<Option<String>> {
        match self.session.metadata().map_err(ort_error)?.custom(key) {
            Some(raw) if !raw.trim().is_empty() => Ok(Some(raw)),
            _ => Ok(None),
        }
    }

    pub fn probe_io(&self) -> Result<ModelIoProbe> {
        let inputs = self
            .session
            .inputs()
            .iter()
            .map(tensor_spec_from_outlet)
            .collect::<Result<Vec<_>>>()?;
        let outputs = self
            .session
            .outputs()
            .iter()
            .map(tensor_spec_from_outlet)
            .collect::<Result<Vec<_>>>()?;
        Ok(ModelIoProbe { inputs, outputs })
    }

    pub fn run_i64_2d(
        &mut self,
        input_name: &str,
        input: ndarray::ArrayView4<'_, f32>,
    ) -> Result<Array2<i64>> {
        let input_tensor = TensorRef::from_array_view(input).map_err(ort_error)?;
        let outputs = self
            .session
            .run(inputs![input_name => input_tensor])
            .map_err(ort_error)?;
        Self::output_i64_2d(outputs, &self.model_path, &self.output_names)
    }

    fn output_i64_2d(
        outputs: ort::session::SessionOutputs<'_>,
        model_path: &str,
        output_names: &[String],
    ) -> Result<Array2<i64>> {
        let output_name = output_names.first().ok_or_else(|| {
            RapidOcrError::Decode(format!(
                "ONNX session has no output names (model={model_path})"
            ))
        })?;
        let output = outputs.get(output_name.as_str()).ok_or_else(|| {
            RapidOcrError::Decode(format!(
                "ONNX session output `{output_name}` not found in run results (model={model_path})"
            ))
        })?;
        let arr = output.try_extract_array::<i64>().map_err(|error| {
            RapidOcrError::Decode(format!(
                "failed to extract output `{output_name}` as i64 tensor (model={model_path}): {error}"
            ))
        })?;
        arr.into_dimensionality::<Ix2>()
            .map(|view| view.to_owned())
            .map_err(|error| {
                RapidOcrError::Decode(format!(
                    "unexpected output rank for model {model_path}: expected rank2: {error}"
                ))
            })
    }

    pub fn provider_resolution(&self) -> ProviderResolution {
        self.provider_resolution
    }

    /// 单输出域用：只取模型声明的**第一个** f32 输出。
    ///
    /// 「只取第一个」在这里是**有意**的语义（普通 OCR 的 det / cls / rec 都是单输出），
    /// 但它对多输出模型是静默错误 —— 需要多路输出时用
    /// [`Self::run_all_f32_view_with`]。
    pub fn run_arrayd_view_with<T, F>(
        &mut self,
        input: ndarray::ArrayView4<'_, f32>,
        f: F,
    ) -> Result<T>
    where
        F: for<'a> FnOnce(ArrayViewD<'a, f32>) -> Result<T>,
    {
        let outputs = Self::run_raw(&mut self.session, input)?;
        let output_name = self.output_names.first().ok_or_else(|| {
            RapidOcrError::Decode(format!(
                "ONNX session has no output names (model={})",
                self.model_path
            ))
        })?;
        let output = outputs.get(output_name.as_str()).ok_or_else(|| {
            RapidOcrError::Decode(format!(
                "ONNX session output `{output_name}` not found in run results (model={})",
                self.model_path
            ))
        })?;
        let arr = output.try_extract_array::<f32>().map_err(|error| {
            RapidOcrError::Decode(format!(
                "failed to extract output `{output_name}` as f32 tensor (model={}): {error}",
                self.model_path
            ))
        })?;
        f(arr.view())
    }

    /// 多输出域用：一次 `run` 的**全部** f32 输出，按模型声明的输出顺序，零拷贝。
    ///
    /// 为什么必须存在这个方法：SLANet 表格结构模型有**两路**输出
    /// （`save_infer_model/scale_0.tmp_0` FLOAT `(N, L, 8)` 单元格框 +
    /// `save_infer_model/scale_1.tmp_0` FLOAT `(N, L, 50)` 结构 token 概率）。
    /// 用 [`Self::run_arrayd_view_with`] 只拿到框、丢掉结构概率，**不会报任何错** ——
    /// 这类静默错误必须由 API 形状挡住，而不是靠调用方记得去读 [`Self::output_names`]。
    ///
    /// `f` 收到的视图借用 ONNX Runtime 的输出缓冲，**生命周期不得逃逸出闭包**：
    /// 需要留存的量必须在闭包内 `to_owned()`（或等价地转成自有数据结构）。
    pub fn run_all_f32_view_with<T, F>(
        &mut self,
        input: ndarray::ArrayView4<'_, f32>,
        f: F,
    ) -> Result<T>
    where
        F: for<'a> FnOnce(&'a [(&'a str, ArrayViewD<'a, f32>)]) -> Result<T>,
    {
        let outputs = Self::run_raw(&mut self.session, input)?;
        let mut views: Vec<(&str, ArrayViewD<'_, f32>)> =
            Vec::with_capacity(self.output_names.len());
        for name in &self.output_names {
            let output = outputs.get(name.as_str()).ok_or_else(|| {
                RapidOcrError::Decode(format!(
                    "ONNX session output `{name}` not found in run results (model={})",
                    self.model_path
                ))
            })?;
            let arr = output.try_extract_array::<f32>().map_err(|error| {
                RapidOcrError::Decode(format!(
                    "failed to extract output `{name}` as f32 tensor (model={}): {error}",
                    self.model_path
                ))
            })?;
            // `try_extract_array` 已经把视图借到 ORT 的输出缓冲上（`ArrayViewD<'outputs, f32>`），
            // 直接把它移进 `views`；再调一次 `.view()` 会把生命周期缩到 `arr` 这个局部变量上。
            views.push((name.as_str(), arr));
        }
        f(&views)
    }

    /// [`Session::run`] 的**唯一**调用点。
    ///
    /// 单输出与多输出两条路径共用这里，是为了避免「两处各写一遍 run + 错误映射」
    /// 那种会随时间漂移的重复。
    fn run_raw<'s>(
        session: &'s mut Session,
        input: ndarray::ArrayView4<'_, f32>,
    ) -> Result<ort::session::SessionOutputs<'s>> {
        let input_tensor = TensorRef::from_array_view(input).map_err(ort_error)?;
        session.run(inputs![input_tensor]).map_err(ort_error)
    }

    pub fn run_array2_view_with<T, F>(
        &mut self,
        input: ndarray::ArrayView4<'_, f32>,
        f: F,
    ) -> Result<T>
    where
        F: for<'a> FnOnce(ArrayView2<'a, f32>) -> Result<T>,
    {
        let model_path = self.model_path.clone();
        self.run_arrayd_view_with(input, |arr| {
            let arr = arr.into_dimensionality::<Ix2>().map_err(|error| {
                RapidOcrError::Decode(format!(
                    "unexpected output rank for model {model_path}: expected rank2: {error}"
                ))
            })?;
            f(arr)
        })
    }

    pub fn run_array3_view_with<T, F>(
        &mut self,
        input: ndarray::ArrayView4<'_, f32>,
        f: F,
    ) -> Result<T>
    where
        F: for<'a> FnOnce(ArrayView3<'a, f32>) -> Result<T>,
    {
        let model_path = self.model_path.clone();
        self.run_arrayd_view_with(input, |arr| {
            let arr = arr.into_dimensionality::<Ix3>().map_err(|error| {
                RapidOcrError::Decode(format!(
                    "unexpected output rank for model {model_path}: expected rank3: {error}"
                ))
            })?;
            f(arr)
        })
    }

    pub fn run_array4_view_with<T, F>(
        &mut self,
        input: ndarray::ArrayView4<'_, f32>,
        f: F,
    ) -> Result<T>
    where
        F: for<'a> FnOnce(ndarray::ArrayView4<'a, f32>) -> Result<T>,
    {
        let model_path = self.model_path.clone();
        self.run_arrayd_view_with(input, |arr| {
            let arr = arr.into_dimensionality::<Ix4>().map_err(|error| {
                RapidOcrError::Decode(format!(
                    "unexpected output rank for model {model_path}: expected rank4: {error}"
                ))
            })?;
            f(arr)
        })
    }
}

/// [`open_session`] 的返回值：ORT 会话、provider 解析结果，以及实际下发给 ONNX Runtime 的
/// `(intra, inter)` 线程数（`None` = 该线程数未配置）。
/// [`open_session`] 的返回值。
///
/// 用命名结构体而不是元组：字段已经到四个，元组在调用点会退化成"第 3 个是什么"的猜谜，
/// 而这里的每一项都要被如实报告出去（线程数、provider 解析、opset）。
struct OpenedSession {
    session: Session,
    provider_resolution: ProviderResolution,
    /// 实际下发给 ONNX Runtime 的 `(intra, inter)` 线程数（`None` = 该线程数未配置）。
    session_threads: (Option<usize>, Option<usize>),
    /// 模型的 ONNX 域 opset；读不到时为 `None`。
    onnx_opset: Option<u32>,
}

/// 打开一个 ORT 会话。
///
/// 线程设置**不在这里推导**：调用
/// [`RuntimeConfig::effective_session_threads`]，那是本 crate 里唯一的线程策略实现
/// （显式值优先；否则按 `auto_tune_threads` 从共享预算推导；两者都没有时 `None`）。
/// 因此“直接构造 `RuntimeConfig` 的公式路径”和“先经过 `RuntimeProfile::plan` 的引擎路径”
/// 得到的是同一套语义。`None` 表示不调用 `with_intra_threads` / `with_inter_threads`，
/// 由 ONNX Runtime 使用自己的默认值。
///
/// 这里刻意**没有**第二份线程推导逻辑。历史实现有一个 `derive_runtime_threads`，它认为
/// `auto_tune_threads = false` 表示“不要自动配置”，而 `runtime::profile` 当时在同样的输入下
/// 无条件算出 `budget` 并当成显式值下发 —— 同一份 `RuntimeConfig` 通过 `RapidOcrEngine`
/// 与通过 `FormulaSession` / `formula_bench` / `formula_eval` 会得到不同线程行为；
/// 反向的偏差（引擎推导而公式路径只做透传、静默拿到 ORT 默认值）同样存在过。
/// 现在两个方向都只经过 [`RuntimeConfig::effective_session_threads`]。
///
/// `dimension_overrides` 是**逐会话**的（理由见 [`DimensionOverride`]），在 provider 之前施加，
/// 因为它是"图怎么编"的一部分：ORT 在 `commit_from_file` 时就做形状推断与优化，
/// 会话建好之后再改已经来不及。
///
/// # 顺序为什么是这样
///
/// 1. **先解析 provider**，因为 [`SessionBuildPolicy`] 是按 selected EP 分支的，而它的两个
///    setter（mem pattern / 执行模式）都必须在 `commit_from_file` **之前**调用。
///    历史实现把 `resolve_execution_providers` 放在最后，只够用来传 EP 链，不够用来分支构建。
/// 2. 再施加形状 override 与线程：它们与 provider 无关，顺序只要求"在 commit 之前"。
/// 3. commit 之后做 DirectML 的 **opset 后置条件**：opset 只有在会话存在之后才读得到
///    （[`Session::opset_for_domain`] 是会话方法），而"读不到"按放行处理。
fn open_session(
    model_path: &Path,
    runtime_cfg: &RuntimeConfig,
    dimension_overrides: &[DimensionOverride],
) -> Result<OpenedSession> {
    // 只有 ONNX Runtime 一种后端：以前这里的单变体 `RuntimeBackend` 检查属于伪抽象，
    // 已随枚举一起删除。provider 差异完全由 `resolve_execution_providers` 表达。
    let mut builder = Session::builder().map_err(ort_error)?;

    let provider_chain = resolve_execution_providers(
        &runtime_cfg.provider_preference,
        runtime_cfg.enable_cpu_mem_arena,
        runtime_cfg.fail_if_provider_unavailable,
    )?;
    builder =
        SessionBuildPolicy::for_provider(provider_chain.resolution.selected_ep).apply(builder)?;

    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_error)?;
    for dimension_override in dimension_overrides {
        builder = builder
            .with_dimension_override(&dimension_override.name, dimension_override.size)
            .map_err(ort_error)?;
    }
    let (intra, inter) = runtime_cfg.effective_session_threads();
    if let Some(intra) = intra {
        builder = builder.with_intra_threads(intra).map_err(ort_error)?;
    }
    if let Some(inter) = inter {
        builder = builder.with_inter_threads(inter).map_err(ort_error)?;
    }

    builder = builder
        .with_execution_providers(provider_chain.providers)
        .map_err(ort_error)?;
    let session = builder.commit_from_file(model_path).map_err(ort_error)?;

    let onnx_opset = session.opset_for_domain(ort::editor::ONNX_DOMAIN);
    if provider_chain.resolution.selected_ep == ResolvedExecutionProvider::DirectMl {
        require_direct_ml_opset(onnx_opset, model_path)?;
    }

    Ok(OpenedSession {
        session,
        provider_resolution: provider_chain.resolution,
        session_threads: (intra, inter),
        onnx_opset,
    })
}

fn tensor_spec_from_outlet(outlet: &ort::value::Outlet) -> Result<TensorSpec> {
    let dtype = outlet.dtype();
    if !dtype.is_tensor() {
        return Err(RapidOcrError::Config(format!(
            "model io `{}` must be a tensor, got `{dtype}`",
            outlet.name()
        )));
    }
    let shape = dtype.tensor_shape().ok_or_else(|| {
        RapidOcrError::Config(format!("model io `{}` has no tensor shape", outlet.name()))
    })?;
    let element_type = dtype.tensor_type().ok_or_else(|| {
        RapidOcrError::Config(format!(
            "model io `{}` has no tensor element type",
            outlet.name()
        ))
    })?;
    // 符号名只能从 `ValueType::Tensor` 的字段里取：`ort` 没有公开 accessor
    // （`tensor_shape` / `tensor_type` 是最接近的两个，但都不返回符号名）。
    let dimension_symbols = match dtype {
        ValueType::Tensor {
            dimension_symbols, ..
        } => dimension_symbols
            .iter()
            .map(|symbol| {
                if symbol.is_empty() {
                    None
                } else {
                    Some(symbol.clone())
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(TensorSpec {
        name: outlet.name().to_string(),
        rank: shape.len(),
        dims: shape.to_vec(),
        dimension_symbols,
        element_type,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DIRECT_ML_MAX_OPSET, OrtSession, ResolvedExecutionProvider, SessionBuildOption,
        SessionBuildPolicy, require_direct_ml_opset,
    };
    use crate::config::{ProviderPreference, RuntimeConfig};

    /// CPU 与 CUDA 必须**一个 setter 都不调用**：它们的默认值就是 ORT 的默认值。
    ///
    /// 这条断言是"这次 provider 分支改动没有影响页级性能"的全部依据 —— 没有它，
    /// "CPU 路径没变"就只是注释里的一句话。
    #[test]
    fn the_cpu_and_cuda_policies_ask_for_no_builder_option_at_all() {
        for ep in [
            ResolvedExecutionProvider::Cpu,
            ResolvedExecutionProvider::Cuda,
        ] {
            let policy = SessionBuildPolicy::for_provider(ep);
            assert_eq!(policy, SessionBuildPolicy::ORT_DEFAULTS, "{ep:?}");
            assert!(
                policy.required_options().is_empty(),
                "{ep:?} must not touch any builder option"
            );
        }
    }

    /// DirectML 的必要条件：关掉 memory pattern、强制顺序执行。
    ///
    /// 这里断言的是**具体的选项值**而不是"非空"：`ORT_SEQUENTIAL` 恰好也是 ORT 的默认值，
    /// 所以"值等于默认值"这件事**不能**用来推断"不必下发"。DirectML 的两项都必须在
    /// [`SessionBuildPolicy::must_be_explicit`] 上被显式要求 —— 这条断言就是那个区分的守卫。
    #[test]
    fn direct_ml_disables_memory_pattern_and_parallel_execution() {
        let policy = SessionBuildPolicy::for_provider(ResolvedExecutionProvider::DirectMl);
        assert!(policy.must_be_explicit);
        assert!(!policy.memory_pattern);
        assert!(!policy.parallel_execution);
        assert_eq!(
            policy.required_options(),
            vec![
                SessionBuildOption::MemoryPattern(false),
                SessionBuildOption::ParallelExecution(false),
            ]
        );

        // 反向对照：默认策略（CPU/CUDA）必须一条 setter 都不调用。没有这一条，
        // 上面那个"两项都在"就可能是 `required_options()` 恒返回两项造成的。
        assert!(
            SessionBuildPolicy::ORT_DEFAULTS
                .required_options()
                .is_empty()
        );
    }

    /// DirectML 的 opset 上限：`20` 放行、`21` 拒绝、读不到时放行。
    #[test]
    fn direct_ml_accepts_opset_20_and_rejects_21() {
        let path = std::path::Path::new("model.onnx");
        assert!(require_direct_ml_opset(Some(DIRECT_ML_MAX_OPSET), path).is_ok());
        // 反向对照：比上限小一档也必须放行，否则上面的"放行"可能只是恒真。
        assert!(require_direct_ml_opset(Some(DIRECT_ML_MAX_OPSET - 1), path).is_ok());

        let error = require_direct_ml_opset(Some(DIRECT_ML_MAX_OPSET + 1), path)
            .expect_err("opset above the DirectML ceiling must be refused");
        let message = error.to_string();
        assert!(
            message.contains("opset 21") && message.contains("DirectML"),
            "the error must name both the offending opset and the provider: {message}"
        );
    }

    /// 读不到 opset **不等于**违反约束：此时报错会拦下本来能跑的模型。
    #[test]
    fn an_unreadable_opset_is_not_a_violation() {
        assert!(require_direct_ml_opset(None, std::path::Path::new("model.onnx")).is_ok());
    }

    /// 真实模型的 opset 必须读得出来。
    ///
    /// 这条同时是 [`require_direct_ml_opset`] 的**输入来源**证明：没有它，那条约束只能靠
    /// 调用方猜一个数字传进来。模型不在仓库内时优雅跳过。
    #[test]
    fn the_real_model_reports_its_opset() {
        let Some(path) = crate::test_support::table_structure_model_path() else {
            return;
        };
        let session = OrtSession::open_unchecked(&path, &RuntimeConfig::default(), &[])
            .expect("the real model must load");
        let opset = session.onnx_opset();
        println!("slanet-plus opset = {opset:?}");
        let opset = opset.expect("the bundled runtime must expose the Model Editor API");
        assert!(
            opset <= DIRECT_ML_MAX_OPSET,
            "the table model must stay inside the DirectML opset ceiling, got {opset}"
        );
    }

    /// `docs/09-table-recognition-execution.md` §13.3："多输出情况下拒绝并行 `Run`"。
    ///
    /// # 这条不变量为什么是**结构**而不是**运行期**检查
    ///
    /// `ort::session::Session` 是 `Send + Sync`（`ort-2.0.0-rc.13/src/session/mod.rs:723-724`），
    /// 所以"共享同一个会话"在类型上是被允许的。真正禁止并发 `Run` 的是**借用的形状**：
    /// `Session::run` 的两个调用点都发生在 `&mut Session` 上（`run_raw` 的 `&'s mut Session`
    /// 参数、`run_i64_2d` 的 `&mut self` 接收者），而 [`OrtSession`] 的每个 `run_*` 入口
    /// 都取 `&mut self` —— 于是对同一个 `OrtSession` 并发调用 `Run` 是一个**编译错误**，
    /// 比任何运行期检查都强。
    ///
    /// 剩下的风险只有一个：将来有人加了一个 `&self` 的 run 入口。所以这里把上面那几句话
    /// 变成对**源码本身**的断言（与 E11 的 G4 静态面同一手法）：
    ///
    /// 1. `.run(inputs` 在 `session.rs` 的**生产代码**里恰好两处（`run_raw` 与 `run_i64_2d`），
    ///    于是不存在"第三处没被下面两条覆盖的 `Run`"；
    /// 2. `run_raw` 的第一个参数是 `&'s mut Session`（覆盖第一处）；
    /// 3. 每个 `pub fn run_` 的接收者都是 `&mut self`（覆盖第二处）。
    ///
    /// 第 1 条只扫 `#[cfg(test)]` 之前的部分：测试代码里可以出现任意字样，而这条不变量
    /// 是关于生产代码的。
    #[test]
    fn every_run_entrance_takes_an_exclusive_borrow() {
        let source = include_str!("session.rs");

        // 探针字符串由碎片拼出：`include_str!` 会把本文件读进来，直接写字面量会让下面每条
        // 断言命中测试自己。
        let test_marker = ["#[cfg", "(test)]"].concat();
        let production = source
            .split(&test_marker)
            .next()
            .expect("`split` always yields at least one part");
        assert!(
            production.len() < source.len(),
            "the scan must be limited to production code, otherwise it reads its own literals"
        );

        let run_call = [".run", "(inputs"].concat();
        assert_eq!(
            production.matches(&run_call).count(),
            2,
            "`{run_call}` must have exactly two call sites: `run_raw` and `run_i64_2d`"
        );
        let raw_signature = ["session: &'s ", "mut ", "Session"].concat();
        assert!(
            production.contains(&raw_signature),
            "`run_raw` must take `{raw_signature}`"
        );

        // 每个 `pub fn run_` 的接收者必须是 `&mut self`。用 `match_indices` 而不是
        // `find(trimmed)`：后者在同一行文本出现两次时会永远取到第一处，于是后面那个
        // 入口根本不会被检查。
        let prefix = ["pub fn ", "run_"].concat();
        let mut entrances = 0;
        for (index, _) in production.match_indices(&prefix) {
            // 只认声明本身（同一行前面只有空白），避免匹配到文档注释里的提及。
            // 方法在 `impl` 块里，所以前面是缩进而不是换行。
            let line_prefix = production[..index].rsplit('\n').next().unwrap_or("");
            if !line_prefix.trim().is_empty() {
                continue;
            }
            entrances += 1;
            // 签名可能跨行，所以从声明处往后看一段，找接收者。
            let signature = &production[index..(index + 200).min(production.len())];
            let name = signature.lines().next().unwrap_or(signature).trim();
            assert!(
                signature.contains("&mut self"),
                "`{name}` must take `&mut self`, otherwise parallel `Run` becomes expressible"
            );
        }
        assert!(
            entrances >= 6,
            "the scan must actually find the run entrances, found {entrances}"
        );
    }

    /// 调用方**唯一**能表达"多个线程共用一个会话"的方式是加锁，而加锁就是串行。
    ///
    /// 这不是在测 `std::sync::Mutex`：它证明的是 DirectML 的"同一会话不得并发 `Run`"
    /// 这条要求在我们的 API 形状下**不可能被违反** —— 共享只能经 `&mut` 取得，而
    /// `&mut` 只能来自 `MutexGuard`。模型不在仓库内时优雅跳过。
    #[test]
    fn serialized_sharing_of_one_session_produces_the_same_outputs() {
        let Some(path) = crate::test_support::table_structure_model_path() else {
            return;
        };
        let config = crate::table::structure::TableStructureConfig::default();
        let session = crate::table::structure::TableStructureSession::open(
            &path,
            &RuntimeConfig::default(),
            &config,
        )
        .expect("the real model must load");
        let session = std::sync::Arc::new(std::sync::Mutex::new(session));
        let side = config.input_side_len;
        let input = ndarray::Array4::<f32>::from_elem((1, 3, side, side), 0.5);

        let run = |session: &std::sync::Arc<
            std::sync::Mutex<crate::table::structure::TableStructureSession>,
        >,
                   input: &ndarray::Array4<f32>| {
            let mut guard = session.lock().expect("the mutex is not poisoned");
            guard
                .run_with(input.view(), |outputs| {
                    Ok(outputs
                        .boxes
                        .iter()
                        .map(|v| v.to_bits())
                        .collect::<Vec<_>>())
                })
                .expect("inference must succeed")
        };

        let sequential = run(&session, &input);
        let shared = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let session = std::sync::Arc::clone(&session);
                    let input = input.clone();
                    scope.spawn(move || run(&session, &input))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("no thread may panic"))
                .collect::<Vec<_>>()
        });

        assert_eq!(shared.len(), 2);
        for observed in &shared {
            assert_eq!(
                observed, &sequential,
                "a serialized shared run must produce the same bits as a sequential one"
            );
        }
    }

    /// `RuntimeConfig` 的 provider 字段与 `ResolvedExecutionProvider` 的映射是本模块的输入，
    /// 这里钉住"默认 = CPU"，避免上面的策略测试在错误的 EP 上通过。
    #[test]
    fn the_default_provider_is_cpu() {
        assert_eq!(
            RuntimeConfig::default().provider_preference,
            ProviderPreference::Cpu
        );
    }
}
