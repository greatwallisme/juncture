# Juncture 快速入门

[English Version / 英文版](../en/getting-started.md)

## 什么是 Juncture？

Juncture 是 LangGraph 状态机框架的 Rust 实现，用于构建 LLM 智能体应用。它保留了核心编程模型 -- `StateGraph` + Pregel 执行引擎 -- 同时利用 Rust 的类型系统实现编译时安全和真正的多核并行。

## 前置要求

- Rust 1.85+（edition 2024）
- 真实 LLM 示例需要：OpenAI API 密钥（或任何 OpenAI 兼容的端点）

## 安装

在 `Cargo.toml` 中添加 Juncture 依赖：

```toml
[dependencies]
juncture = "0.1"
juncture-core = "0.1"
juncture-derive = "0.1"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"
```

## 第一个图

这是一个最小的 Juncture 图，包含两个顺序执行的节点：

```rust
use juncture_core::node::NodeFnUpdate;
use juncture_core::{RunnableConfig, StateGraph};
use juncture_derive::State;

// 使用 #[derive(State)] 定义状态
#[derive(State, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct WorkflowState {
    step: String,
    count: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 创建图构建器
    let mut graph = StateGraph::<WorkflowState>::new();

    // 添加节点
    graph.add_node_simple(
        "greet",
        NodeFnUpdate(|state: &WorkflowState| {
            let count = state.count;
            async move {
                Ok(WorkflowStateUpdate {
                    step: Some("greeted".to_string()),
                    count: Some(count + 1),
                })
            }
        }),
    )?;

    graph.add_node_simple(
        "finish",
        NodeFnUpdate(|state: &WorkflowState| {
            let count = state.count;
            async move {
                Ok(WorkflowStateUpdate {
                    step: Some("done".to_string()),
                    count: Some(count + 1),
                })
            }
        }),
    )?;

    // 定义执行流：greet -> finish
    graph.add_edge("greet", "finish");
    graph.set_entry_point("greet");
    graph.set_finish_point("finish");

    // 编译并执行
    let compiled = graph.compile()?;
    let initial_state = WorkflowState {
        step: "initialized".to_string(),
        count: 0,
    };

    let output = compiled.invoke(initial_state, &RunnableConfig::default())?;
    println!("最终状态: step={}, count={}", output.value.step, output.value.count);
    println!("执行步数: {}", output.metadata.steps);

    Ok(())
}
```

## 运行示例

Juncture 提供了 17 个示例，逐步演示更复杂的模式。

### 模拟示例（无需 API 密钥）

这些示例使用模拟数据，无需外部依赖即可运行：

```bash
# 基本状态机
cargo run -p juncture-simple-example --bin 01_state_machine

# 不同 Reducer 的计数器
cargo run -p juncture-simple-example --bin 02_counter_reducers

# 条件路由
cargo run -p juncture-simple-example --bin 03_conditional_routing

# 使用模拟模型的聊天
cargo run -p juncture-simple-example --bin 04_chat_basic

# 工具调用（手动）
cargo run -p juncture-simple-example --bin 05_tool_calling

# 流式执行
cargo run -p juncture-simple-example --bin 06_streaming

# 人在回路
cargo run -p juncture-simple-example --bin 07_human_in_the_loop

# 检查点与恢复
cargo run -p juncture-simple-example --bin 08_checkpoint_resume

# 错误恢复
cargo run -p juncture-simple-example --bin 09_error_recovery
```

### 真实 LLM 示例（需要 API 密钥）

这些示例调用真实的 LLM API。首先配置环境：

```bash
cp examples/.env.example examples/.env
# 编辑 .env 并设置 OPENAI_API_KEY
```

```bash
# 基本聊天
cargo run -p juncture-simple-example --bin 10_basic_chat

# 流式聊天
cargo run -p juncture-simple-example --bin 11_streaming_chat

# 使用真实 LLM 的工具调用
cargo run -p juncture-simple-example --bin 12_tool_calling

# ReAct 智能体循环
cargo run -p juncture-simple-example --bin 13_react_agent

# 多轮对话
cargo run -p juncture-simple-example --bin 14_multi_turn

# 结构化输出提取
cargo run -p juncture-simple-example --bin 15_structured_output
```

### 深度研究（独立包）

多智能体研究助手，支持网络搜索、子智能体委托和中间件：

```bash
cargo run -p deep-research -- "量子计算的现状是什么？"
cargo run -p deep-research -- --model gpt-4o-mini "解释最新的 AI 突破"
cargo run -p deep-research -- --verbose "研究主题"
```

### 遥测演示

端到端 OpenTelemetry 流水线，集成 Jaeger 和 Prometheus：

```bash
# 启动遥测基础设施
docker compose -f docker/telemetry/docker-compose.yml up -d

# 运行演示
cargo run -p juncture-simple-example --bin telemetry_demo

# 验证：Jaeger UI 地址 http://localhost:16686
```

## 环境配置

真实 LLM 示例通过 `dotenvy` 从 `.env` 加载配置：

```bash
OPENAI_API_KEY=sk-your-key          # 必需
OPENAI_BASE_URL=https://...         # 可选，用于 OpenAI 兼容 API
OPENAI_MODEL=gpt-4o                 # 可选，默认 gpt-4o
TAVILY_API_KEY=tvily-your-key       # 可选，用于深度研究的网络搜索
```

## 构建与测试

```bash
# 构建所有 crate
cargo build --workspace --all-features

# 运行所有测试
cargo test --workspace --all-targets --all-features

# 运行 clippy（强制零警告）
cargo clippy --workspace --all-targets --all-features -- -D warnings

# 检查格式
cargo fmt --all -- --check

# 运行单个测试
cargo test -p juncture-core -- test_name --exact
```

## 工作区结构

```
juncture/              -- 门面 crate（预导入、LLM 提供者、Tool trait、预构建智能体）
juncture-core/         -- Channel 系统、StateGraph、Pregel 引擎、Node/Edge
juncture-derive/       -- #[derive(State)] 过程宏
juncture-checkpoint/   -- CheckpointSaver（MemorySaver、SqliteSaver、PostgresSaver）
juncture-tracing/      -- OpenTelemetry 集成
juncture-store/         -- 跨线程持久化键值存储
benchmarks/            -- 性能对比（Juncture vs LangGraph）
examples/              -- 15 个示例 + 深度研究 + 遥测演示
```

## 下一步

- [核心概念](core-concepts.md) -- 理解 State、StateGraph、Reducer 和 Pregel 引擎
- [示例指南](examples-guide.md) -- 所有 17 个示例的详细讲解
- [高级功能](advanced-features.md) -- 流式、人在回路、检查点、工具、遥测
