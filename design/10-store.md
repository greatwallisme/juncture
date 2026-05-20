# Store（跨线程长期记忆）

<!-- Addresses finding: C-01 -->
<!-- Addresses finding: Part3#4 -->

## 概述

Store 是独立于 Checkpoint 的持久化键值存储，用于跨线程共享长期记忆。

与 Checkpoint 的区别：
- **Checkpoint**：单线程的执行状态快照，跟随图的生命周期
- **Store**：全局共享的键值存储，独立于任何线程或图执行实例

典型场景：
- Agent 跨对话共享用户偏好
- 知识库检索增强
- 工具执行结果的长期缓存
- 用户画像持久化

**源码结构**：
```
juncture-store/
├── src/
│   ├── lib.rs              # 公共导出
│   ├── trait.rs            # Store trait
│   ├── types.rs            # Item, SearchQuery, SearchResult 等
│   ├── memory.rs           # MemoryStore 实现
│   ├── filter.rs           # 过滤操作符
│   ├── vector.rs           # 向量搜索 (feature = "vector")
│   └── error.rs            # StoreError
├── Cargo.toml
└── tests/
```

---

## 1. LangGraph 参考架构

**源码**：`libs/checkpoint/langgraph/store/base/__init__.py`

LangGraph 的 BaseStore 提供以下核心能力：

| 能力 | 描述 |
|------|------|
| 层级命名空间 | 元组形式的路径：`("users", "123", "prefs")` |
| 键值操作 | `get`, `put`, `delete` |
| 搜索 | `search` 支持过滤 + 自然语言向量搜索 |
| 命名空间列举 | `list_namespaces` 支持前缀/后缀匹配和通配符 |
| 批量操作 | `batch` / `abatch` 批量执行多种操作 |
| 向量搜索 | 可选的 embedding 索引，支持相似度检索 |
| TTL | 可选的自动过期机制 |
| 过滤操作符 | `$eq`, `$ne`, `$gt`, `$gte`, `$lt`, `$lte` |

**核心类型**：
- `Item`：存储项，包含 value (dict), key, namespace (tuple), created_at, updated_at
- `SearchItem`：搜索结果，额外包含 score
- `GetOp` / `PutOp` / `SearchOp` / `ListNamespacesOp`：操作类型
- `IndexConfig`：向量索引配置 (dims, embed, fields)
- `TTLConfig`：过期配置 (refresh_on_read, default_ttl, sweep_interval_minutes)

---

## 2. Juncture Store 设计

### 2.1 Store Trait

```rust
use async_trait::async_trait;

/// 跨线程长期记忆存储
///
/// 独立于 Checkpoint，提供层级命名空间的键值存储。
/// 所有线程和图执行实例共享访问。
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// 获取指定命名空间下的单个项
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError>;

    /// 存储或更新项。value 为 None 时删除该项。
    async fn put(
        &self,
        namespace: &str,
        key: &str,
        value: serde_json::Value,
        index: Option<Vec<String>>,
    ) -> Result<(), StoreError>;

    /// 删除指定项
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError>;

    /// 搜索项，支持过滤和可选的向量搜索
    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError>;

    /// 列举命名空间，支持前缀/后缀匹配
    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError>;

    /// 批量执行操作
    async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError>;
}
```

### 2.2 核心数据类型

```rust
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// 存储项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// 层级命名空间路径，如 "users/123/preferences"
    pub namespace: String,
    /// 命名空间内的唯一键
    pub key: String,
    /// 存储的值
    pub value: serde_json::Value,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后更新时间
    pub updated_at: DateTime<Utc>,
}

/// 搜索结果项

> **Implementation Note**: `Item::is_expired()` method provides TTL checking helper.
> Returns true if current time exceeds the optional `expires_at` timestamp, enabling efficient expired item filtering.

/// 搜索结果项（带相似度分数）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchItem {
    #[serde(flatten)]
    pub item: Item,
    /// 相似度分数（仅向量搜索时有效）
    pub score: Option<f64>,
}

/// 搜索查询
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// 命名空间前缀，限定搜索范围
    pub namespace_prefix: String,
    /// 过滤条件
    pub filter: Option<FilterExpr>,
    /// 自然语言查询（启用向量搜索时有效）
    pub query: Option<String>,
    /// 返回数量限制
    pub limit: usize,
    /// 分页偏移
    pub offset: usize,
}

/// 搜索结果
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub items: Vec<SearchItem>,
    pub total_count: usize,
}

/// Store 操作类型
pub enum StoreOp {
    Get { namespace: String, key: String },
    Put {
        namespace: String,
        key: String,
        value: serde_json::Value,
        index: Option<Vec<String>>,
    },
    Delete { namespace: String, key: String },
    Search(SearchQuery),
    ListNamespaces {
        prefix: Option<String>,
        suffix: Option<String>,
        max_depth: Option<usize>,
        limit: Option<usize>,
    },
}

/// Store 操作结果
pub enum StoreResult {
    Item(Option<Item>),
    Items(SearchResult),
    Namespaces(Vec<String>),
    None,
}
```

### 2.3 层级命名空间模型

命名空间使用 `/` 分隔的字符串路径，等价于 LangGraph 的元组路径：

```
LangGraph tuple              Juncture string
───────────────────────────  ──────────────────────
("users", "123", "prefs") → "users/123/prefs"
("documents",)             → "documents"
("cache", "embeddings")    → "cache/embeddings"
```

**匹配规则**：

| 模式 | 含义 |
|------|------|
| `"users"` | 精确匹配 "users" 命名空间 |
| `"users/"` | 以 "users" 为前缀的所有命名空间 |
| `"/prefs"` | 以 "prefs" 为后缀的所有命名空间 |
| `"users/*/prefs"` | 中间层级的通配符 |

### 2.4 注入机制

Store 通过 Runtime 注入到节点中（Runtime 定义见 `02-graph-builder.md`）：

```rust
// Runtime 扩展（在 02-graph-builder.md 中定义）
pub struct Runtime<C: Clone + Send + Sync + 'static> {
    pub context: C,
    pub store: Arc<dyn Store>,        // ← Store 注入
    pub stream_writer: StreamWriter,
    pub execution_info: ExecutionInfo,
    // ...
}

// 节点中访问 Store
async fn my_node(state: MyState, runtime: &Runtime<()>) -> Result<Command<MyState>, JunctureError> {
    // 存储用户偏好
    runtime.store.put(
        "users/123/preferences",
        "theme",
        serde_json::json!({"mode": "dark"}),
        None,
    ).await?;

    // 检索用户偏好
    let prefs = runtime.store.get("users/123/preferences", "theme").await?;

    Ok(Command::update(state_update))
}
```

**工具中的 Store 访问**（InjectedStore 等价）：

```rust
// Tool trait 扩展（见 08-llm-tools.md）
pub trait Tool: Send + Sync + 'static {
    // ... 现有方法

    /// 工具可以请求 Store 访问
    fn requires_store(&self) -> bool {
        false
    }

    /// 带 Store 的执行（由 ToolNode 在 requires_store() 返回 true 时调用）
    fn invoke_with_store(
        &self,
        input: ToolInput,
        store: &dyn Store,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        // 默认委托给 invoke
        self.invoke(input)
    }
}
```

### 2.5 内存实现 (MemoryStore)

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub struct MemoryStore {
    /// namespace -> (key -> Item)
    data: Arc<RwLock<HashMap<String, HashMap<String, Item>>>>,
    /// 可选的向量索引配置
    index_config: Option<IndexConfig>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            index_config: None,
        }
    }

    pub fn with_vector_search(mut self, config: IndexConfig) -> Self {
        self.index_config = Some(config);
        self
    }
}

#[async_trait]
impl Store for MemoryStore {
    // ... 标准 CRUD 实现
}
```

---

## 3. 向量搜索

<!-- Addresses finding: M-13 -->

向量为可选功能，通过 `feature = "vector"` 启用。

### 3.1 索引配置

```rust
/// 向量索引配置
pub struct IndexConfig {
    /// 向量维度
    pub dims: usize,
    /// 文本嵌入函数
    pub embed: Box<dyn EmbeddingFunc>,
    /// 要索引的字段路径（JSON path）
    pub fields: Option<Vec<String>>,
}

/// 嵌入函数 trait
#[async_trait]
pub trait EmbeddingFunc: Send + Sync + 'static {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, StoreError>;
}
```

### 3.2 搜索流程

```
search(query="python tutorial")
  │
  ├─ 1. embed(query) → query_vector: Vec<f32>
  │
  ├─ 2. filter 应用：namespace_prefix + filter 条件
  │
  ├─ 3. 向量相似度计算（cosine similarity）
  │     similarity(a, b) = dot(a, b) / (|a| * |b|)
  │
  ├─ 4. 按 score 降序排序
  │
  └─ 5. limit/offset 分页返回
```

### 3.3 存储时的索引

调用 `put()` 时，如果 Store 配置了向量索引，自动对指定字段进行嵌入：

```rust
async fn put(&self, namespace: &str, key: &str, value: Value, index: Option<Vec<String>>) {
    // 1. 存储 key-value
    // 2. 如果有 vector config：
    //    a. 提取文本（从 value 中按 fields 或 index 参数）
    //    b. embed(texts) → vectors
    //    c. 存储 vectors 到索引
}
```

---

## 4. 过滤操作符

<!-- Addresses finding: M-14 -->

搜索时的过滤条件使用类型化的过滤表达式：

```rust
/// 过滤表达式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterExpr {
    /// 字段等于值
    Eq { field: String, value: serde_json::Value },
    /// 字段不等于值
    Ne { field: String, value: serde_json::Value },
    /// 字段大于值
    Gt { field: String, value: serde_json::Value },
    /// 字段大于等于值
    Gte { field: String, value: serde_json::Value },
    /// 字段小于值
    Lt { field: String, value: serde_json::Value },
    /// 字段小于等于值
    Lte { field: String, value: serde_json::Value },
    /// 逻辑与
    And(Vec<FilterExpr>),
    /// 逻辑或
    Or(Vec<FilterExpr>),
    /// 逻辑非
    Not(Box<FilterExpr>),
}
```


> **Implementation Note**: Complete `FilterExpr::matches()` method with full evaluation engine.
> Supports dot-notation path access (e.g., "metadata.status") and type-aware JSON comparison logic.
**使用示例**：

```rust
let query = SearchQuery {
    namespace_prefix: "documents".to_string(),
    filter: Some(FilterExpr::And(vec![
        FilterExpr::Eq {
            field: "status".to_string(),
            value: json!("active"),
        },
        FilterExpr::Gte {
            field: "score".to_string(),
            value: json!(3.0),
        },
    ])),
    limit: 10,
    ..Default::default()
};
let results = store.search(query).await?;
```

**与 LangGraph 过滤的对应关系**：

| LangGraph | Juncture |
|-----------|----------|
| `{"status": "active"}` | `FilterExpr::Eq { field: "status", value: json!("active") }` |
| `{"score": {"$gt": 4.99}}` | `FilterExpr::Gt { field: "score", value: json!(4.99) }` |
| 多条件 dict (AND) | `FilterExpr::And(vec![...])` |

---

## 5. 批量操作

`batch()` 方法允许在单次调用中执行多种操作：

```rust
let results = store.batch(vec![
    StoreOp::Get { namespace: "users/123".into(), key: "prefs".into() },
    StoreOp::Put {
        namespace: "cache".into(),
        key: "result_456".into(),
        value: json!({"data": "..."}),
        index: None,
    },
    StoreOp::Search(SearchQuery {
        namespace_prefix: "docs".into(),
        query: Some("rust async".into()),
        limit: 5,
        ..Default::default()
    }),
]).await?;
```

MemoryStore 实现中批量操作顺序执行；SQL 后端可优化为单次事务。

---

## 6. 持久化后端

### 6.1 MemoryStore

已在上文定义。适用场景：单进程测试、开发环境。

### 6.2 SQL 后端（设计草图）

```rust
pub struct SqliteStore {
    pool: sqlx::SqlitePool,
    index_config: Option<IndexConfig>,
}

pub struct PostgresStore {
    pool: sqlx::PgPool,
    index_config: Option<IndexConfig>,
}
```

**表结构**：

```sql
CREATE TABLE store_items (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     JSON NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (namespace, key)
);

-- 向量搜索（PostgreSQL + pgvector）
CREATE TABLE store_vectors (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    field     TEXT NOT NULL,
    vector    VECTOR(1536),
    FOREIGN KEY (namespace, key) REFERENCES store_items(namespace, key) ON DELETE CASCADE
);
```

---

## 7. 与 Checkpoint 的关系

| 维度 | Checkpoint | Store |
|------|-----------|-------|
| 生命周期 | 随图执行创建/更新 | 独立持久化 |
| 作用域 | 单线程 (thread_id) | 全局共享 |
| 数据类型 | 完整状态快照 (channel_values) | 键值对 |
| 版本管理 | 多版本 (parent chain) | 单版本 (last write wins) |
| 使用场景 | 执行恢复、time-travel | 跨对话记忆、知识库 |

Store 和 Checkpoint 使用不同的持久化后端，互不影响：
- Store 可用 MemoryStore + PostgresStore
- Checkpoint 可用 MemorySaver + PostgresSaver
- 两者可共享同一个数据库连接池，但使用不同的表

---

## 8. 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("item not found: {namespace}/{key}")]
    NotFound { namespace: String, key: String },

    #[error("invalid namespace: {0}")]
    InvalidNamespace(String),

    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("vector search error: {0}")]
    VectorSearch(String),

    #[error("embedding error: {0}")]
    Embedding(String),
}
```

---

## 9. 实现清单

| 优先级 | 项目 | 依赖 |
|--------|------|------|
| P0 | Store trait 定义 | - |
| P0 | Item / SearchQuery / SearchResult 类型 | - |
| P0 | MemoryStore 基础 CRUD | Store trait |
| P0 | Runtime.store 注入 | Runtime (02-graph-builder.md) |
| P1 | 层级命名空间匹配 | - |
| P1 | 过滤操作符实现 | SearchQuery |
| P1 | 批量操作 | Store trait |
| P2 | 向量搜索 (feature = "vector") | EmbeddingFunc |
| P2 | Tool InjectedStore | Tool trait (08-llm-tools.md) |
| P3 | SqliteStore | sqlx |
| P3 | PostgresStore + pgvector | sqlx, pgvector |
| P3 | TTL 自动过期 | 定时清理任务 |

---

## 9. TTL 与自动过期（<!-- Addresses finding: M-11 -->）

> 参考: `langgraph/libs/checkpoint/langgraph/store/base/__init__.py:545` — TTLConfig

Store 支持 TTL（Time-To-Live）自动过期机制，用于临时缓存和会话数据。

### 9.1 TTLConfig

```rust
/// TTL 配置
#[derive(Clone, Debug)]
pub struct TTLConfig {
    /// 默认 TTL（从创建时间开始计算）
    pub default_ttl: Option<Duration>,
    
    /// 读取时是否刷新 TTL（滑动过期）
    pub refresh_on_read: bool,
    
    /// 清理任务间隔
    pub sweep_interval: Duration,
    
    /// 每次清理的最大数量（避免长时间阻塞）
    pub sweep_max_items: usize,
}

impl Default for TTLConfig {
    fn default() -> Self {
        Self {
            default_ttl: None,
            refresh_on_read: false,
            sweep_interval: Duration::from_secs(300), // 5 分钟
            sweep_max_items: 1000,
        }
    }
}
```

> **Implementation Note**: TTL sweep task automation fully integrated into MemoryStore.
> Background cleanup with configurable intervals via `start_sweep_task()`, respecting `sweep_max_items` to avoid blocking.


### 9.2 Item 扩展

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub namespace: String,
    pub key: String,
    pub value: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    
    /// <!-- Addresses finding: M-11 -->
    /// 过期时间（可选）
    pub expires_at: Option<DateTime<Utc>>,
}
```

### 9.3 过期清理逻辑

```rust
impl<S: Store> MemoryStore<S> {
    /// 后台任务：定期清理过期项
    pub async fn sweep_expired_items(&self) -> Result<usize, StoreError> {
        let now = Utc::now();
        let mut count = 0;
        
        let mut items = self.items.write().await;
        let mut keys_to_remove = Vec::new();
        
        for (key, item) in items.iter() {
            if let Some(expires_at) = item.expires_at {
                if expires_at < now {
                    keys_to_remove.push(key.clone());
                    count += 1;
                    if count >= self.ttl_config.sweep_max_items {
                        break;
                    }
                }
            }
        }
        
        for key in keys_to_remove {
            items.remove(&key);
        }
        
        Ok(count)
    }
    
    /// 启动后台清理任务
    pub fn start_sweep_task(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.ttl_config.sweep_interval);
            loop {
                interval.tick().await;
                if let Err(e) = self.sweep_expired_items().await {
                    tracing::warn!("Store sweep failed: {}", e);
                }
            }
        })
    }
}
```

### 9.4 使用示例

```rust
// 创建带 TTL 的 Store
let mut store = MemoryStore::new();
store.set_ttl_config(TTLConfig {
    default_ttl: Some(Duration::from_secs(3600)), // 1 小时
    refresh_on_read: true,
    ..Default::default()
});

// 存储时设置过期时间
store.put(
    "cache",
    "temp_data",
    json!({"data": "..."}),
    None,
).await?;

// 启动后台清理
let store = Arc::new(store);
let _sweep_handle = store.start_sweep_task();
```

---

## 10. 与 LangGraph 的关键差异

| LangGraph (Python) | Juncture (Rust) | 原因 |
|--------------------|------------------|------|
| `namespace: tuple[str, ...]` | `namespace: String` ("/" 分隔) | Rust 字符串更自然，避免元组泛型 |
| `value: dict[str, Any]` | `value: serde_json::Value` | Rust 需要 JSON 动态类型 |
| `embed: Embeddings \| Callable` | `embed: Box<dyn EmbeddingFunc>` | Rust trait object 替代鸭子类型 |
| `filter: dict[str, Any]` | `filter: Option<FilterExpr>` | 类型安全的过滤表达式 |
| `batch(ops: Iterable[Op])` | `batch(ops: Vec<StoreOp>)` | Rust 所有权模型 |
| `Store` 基类继承 | `Store` trait | Rust 无继承，使用 trait |

---

## 源码参考索引

| Juncture 概念 | LangGraph 源码 |
|---------------|----------------|
| Store trait | `libs/checkpoint/langgraph/store/base/__init__.py` - `BaseStore` |
| Item | `libs/checkpoint/langgraph/store/base/__init__.py:51` - `Item` class |
| SearchQuery | `libs/checkpoint/langgraph/store/base/__init__.py:203` - `SearchOp` |
| 过滤操作符 | `libs/checkpoint/langgraph/store/base/__init__.py:250` - filter 支持 |
| 向量搜索 | `libs/checkpoint/langgraph/store/base/__init__.py:570` - `IndexConfig` |
| 命名空间列举 | `libs/checkpoint/langgraph/store/base/__init__.py:368` - `ListNamespacesOp` |
| MemoryStore | `libs/checkpoint/langgraph/store/memory/__init__.py` - `InMemoryStore` |
| TTL 配置 | `libs/checkpoint/langgraph/store/base/__init__.py:545` - `TTLConfig` |
| 批量操作 | `libs/checkpoint/langgraph/store/base/__init__.py:724` - `batch/abatch` |
