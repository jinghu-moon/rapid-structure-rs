# table-onnx 测试 fixture

本目录的 `*.onnx` 为 KB 级模型契约探针测试件，**随仓库提交**：`.gitignore` 中的
`*.onnx` 规则被 `!tests/fixtures/**/*.onnx` 例外放行，因此干净 clone / CI 可以直接
运行 `src/table/structure.rs` 的契约测试，不需要本机外部模型。

## 文件

| fixture | 作用 |
| --- | --- |
| `table_ok.onnx` | 正向契约：`x float[Dyn,3,Dyn,Dyn]` -> 两路输出 `float[Dyn,4,8]` + `float[Dyn,4,50]`，附**换行分隔**的 `character` metadata |
| `table_single_output.onnx` | 只有框那一路（输出数 = 1） |
| `table_output_dtype.onnx` | 结构概率那一路为 `int64` |
| `table_output_last_dim.onnx` | 结构概率那一路末维为 51 |
| `table_renamed_output.onnx` | 两路改名 `boxes` / `structures`，输出数与 dtype 全对、名字全错（走"缺项"分支） |
| `table_extra_output.onnx` | 两路契约输出齐全，另多一路 `extra`（走"多余输出"分支） |
| `table_metadata_json.onnx` | `character` metadata 为公式域风格的 JSON |
| `table_metadata_absent.onnx` | 缺少 `character` metadata |
| `table_multi_input.onnx` / `table_input_int64.onnx` / `table_input_rank3.onnx` | 违反单输入 / 输入 dtype / 输入 rank |

## 与真实模型的对照

`models/table/slanet-plus.onnx`（7,758,305 B，未入库）实测：

| | 真实模型 | 本目录 fixture |
| --- | --- | --- |
| 输入 | `x` FLOAT `[Dyn,3,Dyn,Dyn]` | 同（`table_ok.onnx`） |
| 输出 1 | `save_infer_model/scale_0.tmp_0` FLOAT `[Dyn,Dyn,8]` | `[Dyn,4,8]` |
| 输出 2 | `save_infer_model/scale_1.tmp_0` FLOAT `[Dyn,Dyn,50]` | `[Dyn,4,50]` |
| 解码长度 `L` | **动态**（实测 L ≤ 500） | 固定 4 |
| `ir_version` / opset | 7 / 14 | 8 / 15 |
| `character` metadata | 48 token、582 字符、换行分隔 | **逐字节相同** |

`L` 是刻意固定的：契约只约束 rank / dtype / **末维**，不约束 `L`。
若把 `L` 也写成固定值，反而会在 E5 固定形状时给出错误的信心。

`character` metadata 是**逐字节照抄**真实模型的值（脚本里带
`assert len(REAL_CHARACTER_TOKENS) == 48` 与
`assert len(SYNTHETIC_CHARACTER_METADATA) == 582`），因此在没有真实模型的干净 clone 中，
fixture 仍然代表生产配置的 metadata 形状。

### 一个容易看反的事实

SLANet 的结构 token **不是整标签，而是片段**：

```
<td>              = <td  +  >                        (token 6 + token 7)
<td colspan="2">  = <td  +   colspan="2"  +  >       (token 6 + token 9 + token 7)
空单元格           = <td></td>                        (token 47，单 token)
```

所以词表里**同时**有 `<td`（token 6）、`>`（token 7）、`</td>`（token 8）和
`<td></td>`（token 47）；「不含 `<td>`」只对**完整的** `<td>` 成立。
解码器必须先拼接片段再解析 HTML，不能按 token 做字符串相等匹配。

`rowspan` / `colspan` 都从 **2** 起（1 隐含，不需要 token），到 20 止。

## 重建

依赖 Python `onnx` 与 `onnxruntime`：

```powershell
python tools/build_table_onnx_fixtures.py
```

脚本**不依赖任何外部模型**、不依赖开发机绝对路径，重跑即可得到逐字节相同的 11 个
fixture。`check()` 会对每个 fixture 跑 `onnx.checker.check_model` 并用
`CPUExecutionProvider` 实际执行一遍，打印 shape/dtype；设计上不可运行的负例只打印
`rejected as designed`。

## 验证

```powershell
cargo test --lib table::structure
```

必须在**没有任何模型环境变量**的干净 clone 中通过。
