# 09 - 可观测性与追踪

## 概述

Juncture 的可观测性基于 Rust 生态标准栈 `tracing` + `opentelemetry`，提供零配置自动插桩。设计原则：

- 自动化：框架自动为每个节点执行、LLM 调用、工具调用生成 span，用户无需手动埋点
- 标准化：完全基于 OpenTelemetry 协议，对接任何 OTLP 兼容后端（Jaeger、Datadog、Grafana Tempo）
- 无供应商锁定：不绑定 LangSmith 或任何商业监控平台
- 分层可用：即使不启用 OTLP 导出，基础 tracing 日志始终可用

---

## 1. Span 层次结构

每次图执行自动生成如下 span 树：

```
juncture.graph.invoke [thread_id="t1", graph="react_agent"]
├── juncture.superstep [step=0]
│   └── juncture.node.execute [node="agent"]
│       └── juncture.llm.call [model="claude-sonnet-4-20250514", tokens.in=150, tokens.out=85]
├── juncture.superstep [step=1]
│   └── juncture.node.execute [node="tools"]
│       ├── juncture.tool.call [tool="search", duration_ms=230]
│       └── juncture.tool.call [tool="calculator", duration_ms=5]
── juncture.superstep [step=2]
│   └── juncture.node.execute [node="agent"]
│       └── juncture.llm.call [model="claude-sonnet-4-20250514", tokens.in=320, tokens.out=45]
├── juncture.checkpoint.put [checkpoint_id="abc123", step=2]
└── juncture.graph.complete [total_steps=3, total_tokens=600, cost_usd=0.0082]
```

**Span 命名规范**：`juncture.{component}.{action}`

---

## 2. Span 属性定义

### 2.1 Graph 级别

| 属性 | 类型 | 说明 |
|---|---|---|
| `juncture.thread.id` | string | 当前执行的 thread_id |
| `juncture.graph.name` | string | 图名称（compile 时指定） |
| `juncture.run.id` | string | 本次执行的唯一 ID |
| `juncture.recursion.limit` | int | 配置的递归限制 |

### 2.2 Superstep 级别

| 属性 | 类型 | 说明 |
|---|---|---|
| `juncture.step` | int | 当前 superstep 编号 |
| `juncture.step.nodes` | string[] | 本 superstep 执行的节点列表 |
| `juncture.step.duration_ms` | int | superstep 总耗时 |

### 2.3 Node 级别

| 属性 | 类型 | 说明 |
|---|---|---|
| `juncture.node.name` | string | 节点名称 |
| `juncture.node.duration_ms` | int | 节点执行耗时 |
| `juncture.node.error` | string | 错误信息（仅失败时） |
| `juncture.node.output_type` | string | 输出类型：update/command/interrupt |

### 2.4 LLM 调用级别

| 属性 | 类型 | 说明 |
|---|---|---|
| `juncture.llm.model` | string | 模型名称 |
| `juncture.llm.provider` | string | Provider 类型（anthropic/openai/ollama） |
| `juncture.tokens.input` | int | 输入 token 数 |
| `juncture.tokens.output` | int | 输出 token 数 |
| `juncture.cost.usd` | float | 本次调用费用 |
| `juncture.llm.has_tool_calls` | bool | 是否包含工具调用 |
| `juncture.llm.stop_reason` | string | 停止原因 |

### 2.5 Tool 调用级别

| 属性 | 类型 | 说明 |
|---|---|---|
| `juncture.tool.name` | string | 工具名称 |
| `juncture.tool.duration_ms` | int | 工具执行耗时 |
| `juncture.tool.error` | string | 错误信息（仅失败时） |

### 2.6 Checkpoint 级别

| 属性 | 类型 | 说明 |
|---|---|---|
| `juncture.checkpoint.id` | string | checkpoint UUID |
| `juncture.checkpoint.source` | string | input/loop/interrupt |
| `juncture.checkpoint.step` | int | 对应的 superstep |

---

## 3. 集成配置

### 3.1 一行初始化

```rust
juncture::tracing::init()
    .with_otlp_endpoint("http://collector:4317")
    .with_service_name("my-agent-service")
    .install()?;
```

### 3.2 完整配置

```rust
juncture::tracing::init()
    .with_otlp_endpoint("http://collector:4317")
    .with_service_name("my-agent-service")
    .with_service_version("1.0.0")
    .with_resource_attributes([
        ("deployment.environment", "production"),
        ("service.instance.id", "pod-abc123"),
    ])
    .with_trace_sampling(0.1)  // 10% 采样率
    .with_metrics(true)         // 启用 metrics 导出
    .with_log_level(tracing::Level::INFO)
    .install()?;
```

### 3.3 仅日志模式（无 OTLP）

不启用 `tracing` feature 时，框架仍通过 `tracing` crate 输出结构化日志：

```rust
// 用户只需初始化 tracing subscriber
tracing_subscriber::fmt()
    .with_env_filter("juncture=info")
    .init();
```

输出示例：
```
INFO juncture::pregel: graph invoke started thread_id="t1"
INFO juncture::pregel: superstep complete step=0 nodes=["agent"] duration_ms=1200
INFO juncture::llm: llm call complete model="claude-sonnet-4-20250514" tokens.in=150 tokens.out=85
INFO juncture::pregel: graph invoke complete steps=3 total_tokens=600
```

---

## 4. Metrics（OpenTelemetry Metrics）

### 4.1 Counter 指标

| 指标名 | 类型 | 标签 | 说明 |
|---|---|---|---|
| `juncture.graph.invocations` | Counter | graph_name | 图执行次数 |
| `juncture.graph.errors` | Counter | graph_name, error_type | 图执行错误次数 |
| `juncture.llm.tokens.input` | Counter | model, provider | 累计输入 token |
| `juncture.llm.tokens.output` | Counter | model, provider | 累计输出 token |
| `juncture.llm.cost_usd` | Counter | model, provider | 累计费用 |
| `juncture.llm.calls` | Counter | model, provider | LLM 调用次数 |
| `juncture.tool.calls` | Counter | tool_name | 工具调用次数 |
| `juncture.tool.errors` | Counter | tool_name | 工具调用失败次数 |
| `juncture.checkpoint.writes` | Counter | source | checkpoint 写入次数 |

### 4.2 Histogram 指标

| 指标名 | 类型 | 标签 | 说明 |
|---|---|---|---|
| `juncture.graph.duration_ms` | Histogram | graph_name | 图执行耗时分布 |
| `juncture.node.duration_ms` | Histogram | node_name | 节点执行耗时分布 |
| `juncture.llm.duration_ms` | Histogram | model | LLM 调用耗时分布 |
| `juncture.llm.tokens_per_call` | Histogram | model | 每次调用 token 数分布 |
| `juncture.tool.duration_ms` | Histogram | tool_name | 工具执行耗时分布 |
| `juncture.superstep.duration_ms` | Histogram | graph_name | superstep 耗时分布 |

### 4.3 Gauge 指标

| 指标名 | 类型 | 说明 |
|---|---|---|
| `juncture.graph.active_invocations` | Gauge | 当前正在执行的图数量 |
| `juncture.budget.remaining_tokens` | Gauge | 剩余 token 预算 |
| `juncture.budget.remaining_cost_usd` | Gauge | 剩余费用预算 |

---

## 4.4 <!-- Addresses finding: M-9 --> Explicit Metrics API

除了 OpenTelemetry 的自动 metrics 采集，Juncture 提供显式 metrics API 用于自定义指标：

```rust
/// Metrics 注册表
///
/// 提供显式创建和注册自定义 metrics 的 API。
pub struct MetricsRegistry {
    /// OpenTelemetry Meter
    meter: opentelemetry::metrics::Meter,
}

impl MetricsRegistry {
    /// 创建新的 Counter metric
    pub fn counter<F>(
        &self,
        name: impl Into<String>,
        f: F,
    ) -> Counter<u64>
    where
        F: FnOnce(&CounterBuilder) -> CounterBuilder,
    {
        let builder = self.meter.u64_counter(name.into());
        let builder = f(builder);
        builder.build()
    }

    /// 创建新的 Histogram metric
    pub fn histogram<F>(
        &self,
        name: impl Into<String>,
        f: F,
    ) -> Histogram<f64>
    where
        F: FnOnce(&HistogramBuilder) -> HistogramBuilder,
    {
        let builder = self.meter.f64_histogram(name.into());
        let builder = f(builder);
        builder.build()
    }

    /// 创建新的 Gauge metric
    pub fn gauge<F>(
        &self,
        name: impl Into<String>,
        f: F,
    ) -> Gauge<f64>
    where
        F: FnOnce(&GaugeBuilder) -> GaugeBuilder,
    {
        let builder = self.meter.f64_gauge(name.into());
        let builder = f(builder);
        builder.build()
    }
}

// > **Implementation Note (C-09-5)**: The actual implementation uses a handle-based abstraction instead
// > of direct OTel types. `CounterHandle`, `HistogramHandle`, and `GaugeHandle` wrapper types
// > (`metrics.rs:16-163`) wrap in-memory storage (HashMap) rather than OpenTelemetry Meter primitives.
// > This provides a cleaner API for in-memory metrics while maintaining the same method signatures
// > (`.increment()`, `.record()`, `.set()`). Future integration with a real OTel Meter will swap the
// > internal storage without changing the handle API.

/// 使用示例
let registry = juncture::metrics::registry();

// 自定义 Counter
let custom_invocations = registry.counter("my_app.custom_invocations", |b| {
    b.with_description("Custom invocation counter")
        .with_unit("1")
});

// 自定义 Histogram
let custom_latency = registry.histogram("my_app.custom_latency_ms", |b| {
    b.with_description("Custom operation latency")
        .with_unit("ms")
        .with_boundaries(vec![1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0])
});

// 在节点中使用
custom_invocations.add(1, &[KeyValue::new("operation", "my_operation")]);
custom_latency.record(duration.as_millis() as f64, &[KeyValue::new("operation", "my_operation")]);
```

---

## 5. Debug 模式

### 5.1 StreamMode::Debug

当使用 `StreamMode::Debug` 时，图执行过程中的所有内部事件通过 stream 输出：

```rust
#[derive(Clone, Debug, Serialize)]
pub enum DebugEvent {
    GraphStart {
        thread_id: String,
        input: serde_json::Value,
    },
    SuperstepStart {
        step: usize,
        pending_nodes: Vec<String>,
    },
    NodeStart {
        node: String,
        step: usize,
    },
    NodeEnd {
        node: String,
        step: usize,
        duration_ms: u64,
        output_type: String,
    },
    NodeError {
        node: String,
        step: usize,
        error: String,
    },
    ChannelWrite {
        channel: String,
        node: String,
        value_summary: String,
    },
    ChannelUpdate {
        channel: String,
        new_version: u64,
    },
    Merge {
        step: usize,
        channels_updated: Vec<String>,
    },
    EdgeTraversed {
        from: String,
        to: String,
        edge_type: String,
    },
    CheckpointSaved {
        checkpoint_id: String,
        step: usize,
        source: String,
    },
    BudgetCheck {
        tokens_used: u64,
        cost_usd: f64,
        budget_remaining_pct: f32,
    },
    GraphEnd {
        total_steps: usize,
        total_duration_ms: u64,
    },
}
```

### 5.2 使用场景

Debug 模式适用于开发阶段，无需配置 OTLP 后端即可观察完整执行流程：

```rust
let mut stream = app.stream(input, &config, StreamMode::Debug).await?;
while let Some(event) = stream.next().await {
    match event? {
        StreamEvent::Debug(debug) => {
            println!("{:?}", debug);
        }
        StreamEvent::End { output } => break,
        _ => {}
    }
}
```

---

## 6. 实现细节

### 6.1 自动插桩位置

框架在以下位置自动创建 span（用户代码无需任何修改）：

| 位置 | Span 名称 | 触发条件 |
|---|---|---|
| `CompiledGraph::invoke` | `juncture.graph.invoke` | 每次 invoke/stream 调用 |
| `PregelLoop::execute_superstep` | `juncture.superstep` | 每个 superstep |
| `PregelLoop::execute_node` | `juncture.node.execute` | 每个节点执行 |
| `ChatModel::invoke/stream` | `juncture.llm.call` | 每次 LLM 调用 |
| `Tool::invoke` | `juncture.tool.call` | 每次工具调用 |
| `CheckpointSaver::put` | `juncture.checkpoint.put` | 每次 checkpoint 写入 |

### 6.2 实现模式

```rust
// Pregel 执行器中的插桩示例
async fn execute_node<S: State>(
    node: &dyn NodeExecutor<S>,
    state: &S,
    ctx: &NodeContext,
) -> Result<Command<S>, JunctureError> {
    let span = tracing::info_span!(
        "juncture.node.execute",
        "juncture.node.name" = node.name(),
        "juncture.step" = ctx.step,
        "juncture.thread.id" = ctx.thread_id(),
        "juncture.node.duration_ms" = tracing::field::Empty,
        "juncture.node.error" = tracing::field::Empty,
        "otel.status_code" = tracing::field::Empty,
    );

    let start = Instant::now();
    let result = node.process(state, ctx).instrument(span.clone()).await;
    let duration = start.elapsed().as_millis() as u64;

    span.record("juncture.node.duration_ms", duration);

    match &result {
        Ok(_) => {
            span.record("otel.status_code", "OK");
        }
        Err(e) => {
            span.record("juncture.node.error", e.to_string().as_str());
            span.record("otel.status_code", "ERROR");
        }
    }

    result
}
```

### 6.3 Feature Gate 策略

| Feature | 提供的能力 | 额外依赖 |
|---|---|---|
| 始终可用 | `tracing` 结构化日志、Debug StreamMode | `tracing` |
| `feature = "otel"` | OTLP trace/metrics 导出 | `tracing-opentelemetry`, `opentelemetry`, `opentelemetry-otlp`, `opentelemetry_sdk` |

不启用 `otel` feature 时：
- span 仍然创建（`tracing` crate 的零成本性——无 subscriber 时 span 是 no-op）
- 用户可用 `tracing-subscriber` 输出到 stdout/文件
- Debug StreamMode 正常工作

启用 `otel` feature 时：
- 额外提供 `juncture::tracing::init()` 配置 API
- span 自动导出到 OTLP 后端
- metrics 自动导出

### 6.4 Context 传播

跨异步边界的 span context 通过 `tracing::Instrument` trait 传播：

```rust
// 并发节点执行时，每个 spawn 的 task 继承父 span
let span = tracing::info_span!("juncture.node.execute", node = name);
join_set.spawn(
    async move {
        node.process(state, ctx).await
    }
    .instrument(span)
);
```

跨进程边界（如 subgraph 在独立服务中执行）时，通过 OpenTelemetry propagator 注入/提取 trace context。

---

## 7. 测试可观测性

### 7.1 Span 断言

使用 `tracing-test` 在集成测试中验证 span 结构：

```rust
#[tokio::test]
async fn test_node_spans_created() {
    let (subscriber, handle) = tracing_test::subscriber::mock()
        .new_span(expect::span().named("juncture.graph.invoke"))
        .new_span(expect::span().named("juncture.superstep").with_field(
            expect::field("juncture.step").with_value(&0u64),
        ))
        .new_span(expect::span().named("juncture.node.execute").with_field(
            expect::field("juncture.node.name").with_value(&"agent"),
        ))
        .done()
        .run_with_handle();

    let _guard = tracing::subscriber::set_default(subscriber);

    let app = build_test_graph().compile_ephemeral().unwrap();
    app.invoke(input, &config).await.unwrap();

    handle.assert_finished();
}
```

### 7.2 Metrics 断言

```rust
#[tokio::test]
async fn test_token_metrics_reported() {
    let metrics = TestMetricsCollector::new();
    let app = build_test_graph()
        .compile_with_metrics(metrics.clone())
        .unwrap();

    app.invoke(input, &config).await.unwrap();

    assert_eq!(metrics.get_counter("juncture.llm.calls"), 2);
    assert!(metrics.get_counter("juncture.llm.tokens.input") > 0);
}
```

> **Implementation Note (C-09-3)**: The `TestMetricsCollector` is more comprehensive than the design
> suggests. It provides dedicated methods for all three metric types: `get_counter(name) -> u64`,
> `get_histogram_samples(name) -> Vec<f64>`, and `get_gauge_value(name) -> Option<f64>`. It also
> tracks metric attributes/labels, allowing tests to assert on labeled metrics (e.g.,
> `get_counter_with_labels("juncture.llm.calls", &[("model", "gpt-4")])`). The collector uses
> `Arc<Mutex<HashMap>>` internally for thread-safe metric accumulation during async tests.

---

## 8. 与 Budget 系统的协同

可观测性系统与 Budget 系统共享 token/cost 数据源：

```
LLM Provider 调用完成
    │
    ├──→ BudgetTracker.report_usage()  (预算检查)
    │
    └──→ Span.record("juncture.tokens.input", N)  (可观测性)
         Span.record("juncture.cost.usd", cost)
         Metrics.add("juncture.llm.tokens.input", N)
```

两者从同一个 `TokenUsage` 数据点获取信息，不重复计算。

---

## 9. 模块文件结构

```
crates/juncture-tracing/src/
├── lib.rs              # 导出 init(), TracingConfig
├── config.rs           # TracingConfig builder
├── spans.rs            # Span 名称常量、属性 key 常量
├── metrics.rs          # Metrics 定义与注册
├── debug.rs            # DebugEvent 类型定义
└── test_utils.rs       # TestMetricsCollector 等测试辅助
```

或者作为 `juncture-core` 的子模块（如果不单独成 crate）：

```
crates/juncture-core/src/
└── tracing/
    ├── mod.rs
    ├── spans.rs
    └── metrics.rs

crates/juncture/src/
└── tracing/
    ├── mod.rs          # init() API（依赖 opentelemetry）
    └── config.rs
```

---

## 10. 对接示例

### Jaeger

```rust
juncture::tracing::init()
    .with_otlp_endpoint("http://localhost:4317")
    .with_service_name("my-agent")
    .install()?;
```

### Datadog

```rust
juncture::tracing::init()
    .with_otlp_endpoint("http://datadog-agent:4317")
    .with_service_name("my-agent")
    .with_resource_attributes([
        ("env", "production"),
        ("version", "1.2.3"),
    ])
    .install()?;
```

### Grafana Tempo + Prometheus

```rust
juncture::tracing::init()
    .with_otlp_endpoint("http://tempo:4317")
    .with_service_name("my-agent")
    .with_metrics(true)  // metrics 导出到 Prometheus
    .with_metrics_endpoint("http://prometheus-pushgateway:9091")
    .install()?;
```

