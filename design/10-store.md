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

**实现细节**：

完整的命名空间管理支持：
- **max_depth 限制**：`list_namespaces()` 支持最大深度截断，防止深层嵌套查询
- **分页支持**：完整的 offset/limit 分页机制，支持大规模命名空间遍历
- **前缀/后缀过滤**：
  - MemoryStore：使用字符串前缀/后缀匹配和 HashSet 去重
  - SQL 后端：通过 `LIKE` 和 `SUBSTR` 查询实现高性能过滤
- **命名空间规范化**：自动处理 `/` 前缀和连续分隔符，确保路径一致性

**实现示例**：

```rust
// MemoryStore 实现片段
impl MemoryStore {
    async fn list_namespaces(
        &self,
        prefix: Option<&str>,
        suffix: Option<&str>,
        max_depth: Option<usize>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<String>, StoreError> {
        let items = self.data.read().await;
        let mut namespaces: HashSet<String> = HashSet::new();

        for (ns, _) in items.iter() {
            // 前缀过滤
            if let Some(p) = prefix {
                if !ns.starts_with(p) {
                    continue;
                }
            }
            // 后缀过滤
            if let Some(s) = suffix {
                if !ns.ends_with(s) {
                    continue;
                }
            }
            // 深度限制
            if let Some(depth) = max_depth {
                let actual_depth = ns.matches('/').count() + 1;
                if actual_depth > depth {
                    continue;
                }
            }
            namespaces.insert(ns.clone());
        }

        // 分页处理
        let mut result: Vec<String> = namespaces.into_iter().collect();
        result.sort();
        if let Some(off) = offset {
            result = result.into_iter().skip(off).collect();
        }
        if let Some(lim) = limit {
            result.truncate(lim);
        }
        Ok(result)
    }
}
```

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

> **Implementation Note (B-10-003):** SQL backends (SqliteStore, PostgresStore) do not yet support vector search. The `Item.embedding` field always returns `None` for SQL backends because no embedding column exists in the SQL schema. Only `MemoryStore` computes and returns embeddings. SQL vector search (pgvector) is deferred to P3 per the implementation roadmap.

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

> **Implementation Note (C-10-1b)**: ~~Standalone crate `FilterExpr` is missing `#[serde(tag = "op")]` on the enum,
> causing serialization format inconsistency with the core implementation.~~ **CORRECTED**: Implementation correctly includes `#[serde(tag = "op")]` with tagged serialization format. Design note was outdated; the standalone crate and core implementation now use consistent tagged enum serialization.

> **Implementation Note (C-10-1)**: Core implementation uses struct variants `And { expressions }` instead of
> tuple variants `And(Vec)`. Standalone crate uses tuple variants. Serialization format differs between the two.
>
> **Implementation Note (D-10-3)**: `And` and `Or` use struct variants with `expressions: Vec<FilterExpr>` field instead of tuple variants `And(Vec<FilterExpr>)`, for better readability and forward compatibility.
> **Implementation Note (C-10-2)**: Core Store trait omits `Debug` bound (claims impossibility with async traits);
> standalone crate successfully implements `Debug` for `Store` trait objects.

### 4.1 完整评估引擎

生产就绪的过滤表达式评估引擎，支持：

**字段路径解析**：
- 点记法支持：`"metadata.status"` 访问嵌套 JSON 字段
- 数组索引：`"items.0.price"` 访问数组元素
- 路径验证：自动处理不存在字段，返回 false 而非错误

**类型感知比较**：
```rust
impl FilterExpr {
    /// 评估过滤表达式是否匹配给定的值
    pub fn matches(&self, value: &serde_json::Value) -> bool {
        match self {
            FilterExpr::Eq { field, value: target } => {
                if let Some(v) = get_nested_field(value, field) {
                    compare_json(&v, target, ComparisonOp::Eq)
                } else {
                    false
                }
            }
            FilterExpr::Gt { field, value: target } => {
                if let Some(v) = get_nested_field(value, field) {
                    compare_json(&v, target, ComparisonOp::Gt)
                } else {
                    false
                }
            }
            // ... 其他操作符
            FilterExpr::And(exprs) => exprs.iter().all(|e| e.matches(value)),
            FilterExpr::Or(exprs) => exprs.iter().any(|e| e.matches(value)),
            FilterExpr::Not(expr) => !expr.matches(value),
        }
    }
}

/// 点记法字段提取
fn get_nested_field(value: &Value, path: &str) -> Option<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for part in parts {
        match current {
            Value::Object(map) => {
                current = map.get(part)?;
            }
            Value::Array(arr) => {
                let idx = part.parse::<usize>().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }

    Some(current.clone())
}

/// 类型感知 JSON 比较
fn compare_json(left: &Value, right: &Value, op: ComparisonOp) -> bool {
    match (left, right) {
        (Value::String(a), Value::String(b)) => match op {
            ComparisonOp::Eq => a == b,
            ComparisonOp::Gt => a > b,
            ComparisonOp::Lt => a < b,
            // ...
        },
        (Value::Number(a), Value::Number(b)) => {
            let a_f = a.as_f64().unwrap();
            let b_f = b.as_f64().unwrap();
            match op {
                ComparisonOp::Eq => a_f == b_f,
                ComparisonOp::Gt => a_f > b_f,
                ComparisonOp::Gte => a_f >= b_f,
                ComparisonOp::Lt => a_f < b_f,
                ComparisonOp::Lte => a_f <= b_f,
                ComparisonOp::Ne => a_f != b_f,
            }
        }
        (Value::Bool(a), Value::Bool(b)) => match op {
            ComparisonOp::Eq => a == b,
            ComparisonOp::Ne => a != b,
            _ => false,
        },
        _ => false,
    }
}
```

### 4.2 SQL 翻译

完整的 SQL 后端支持，自动将 `FilterExpr` 翻译为 WHERE 子句：

```rust
impl FilterExpr {
    /// 转换为 SQL WHERE 子句和参数
    pub fn to_sql(&self) -> (String, Vec<serde_json::Value>) {
        match self {
            FilterExpr::Eq { field, value } => {
                let sql_field = field_to_sql_column(field); // "metadata.status" -> "json_extract(value, '$.metadata.status')"
                (format!("{} = ?", sql_field), vec![value.clone()])
            }
            FilterExpr::And(exprs) => {
                let parts: Vec<_> = exprs.iter().map(|e| e.to_sql()).collect();
                let sql = parts.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>().join(" AND ");
                let params: Vec<_> = parts.into_iter().flat_map(|(_, p)| p).collect();
                (format!("({})", sql), params)
            }
            // ... 其他操作符
        }
    }
}

// 字段路径到 SQL 表达式
fn field_to_sql_column(path: &str) -> String {
    if path.contains('.') {
        format!("json_extract(value, '$.{}')", path)
    } else {
        format!("json_extract(value, '$.{}')", path)
    }
}
```

**布尔值参数序列化**：
- SQLite 的布尔值通过 `i32` (0/1) 正确处理
- PostgreSQL 使用原生 `BOOL` 类型
- 自动类型转换确保查询参数匹配

### 4.3 测试覆盖

完整的测试套件验证所有操作符和边界条件：

```rust
#[cfg(test)]
mod filter_tests {
    use super::*;

    #[test]
    fn test_eq_simple() {
        let value = json!({"status": "active"});
        let expr = FilterExpr::Eq {
            field: "status".into(),
            value: json!("active"),
        };
        assert!(expr.matches(&value));
    }

    #[test]
    fn test_nested_field() {
        let value = json!({"metadata": {"status": "active"}});
        let expr = FilterExpr::Eq {
            field: "metadata.status".into(),
            value: json!("active"),
        };
        assert!(expr.matches(&value));
    }

    #[test]
    fn test_numeric_comparison() {
        let value = json!({"score": 85.5});
        let expr = FilterExpr::Gte {
            field: "score".into(),
            value: json!(80.0),
        };
        assert!(expr.matches(&value));
    }

    #[test]
    fn test_and_combination() {
        let value = json!({"status": "active", "score": 85});
        let expr = FilterExpr::And(vec![
            FilterExpr::Eq { field: "status".into(), value: json!("active") },
            FilterExpr::Gte { field: "score".into(), value: json!(80) },
        ]);
        assert!(expr.matches(&value));
    }

    #[test]
    fn test_missing_field() {
        let value = json!({"name": "test"});
        let expr = FilterExpr::Eq {
            field: "status".into(),
            value: json!("active"),
        };
        assert!(!expr.matches(&value));
    }
}
```

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

### 6.2 SQL 后端（完整实现）

#### 6.2.1 结构定义

```rust
pub struct SqliteStore {
    pool: sqlx::SqlitePool,
    index_config: Option<IndexConfig>,
    ttl_config: TTLConfig,
}

pub struct PostgresStore {
    pool: sqlx::PgPool,
    index_config: Option<IndexConfig>,
    ttl_config: TTLConfig,
}
```

#### 6.2.2 表结构

```sql
CREATE TABLE store_items (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     JSONB NOT NULL,           -- PostgreSQL 使用 JSONB
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,             -- TTL 支持
    PRIMARY KEY (namespace, key)
);

-- SQLite 版本（使用 JSON）
CREATE TABLE store_items (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     TEXT NOT NULL,            -- JSON 字符串
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,                    -- ISO 8601 字符串
    PRIMARY KEY (namespace, key)
);

-- 向量搜索（PostgreSQL + pgvector，P3 待实现）
CREATE TABLE store_vectors (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    field     TEXT NOT NULL,
    vector    VECTOR(1536),
    FOREIGN KEY (namespace, key) REFERENCES store_items(namespace, key) ON DELETE CASCADE
);
```

#### 6.2.3 CRUD 操作

**Get 操作**：
```rust
#[async_trait]
impl Store for SqliteStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, Option<String>)>(
            "SELECT namespace, key, value, created_at, updated_at, expires_at
             FROM store_items WHERE namespace = ? AND key = ?"
        )
        .bind(namespace)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((ns, k, v, created, updated, expires)) = row {
            let item = Item {
                namespace: ns,
                key: k,
                value: serde_json::from_str(&v)?,
                created_at: DateTime::parse_from_rfc3339(&created)?.with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339(&updated)?.with_timezone(&Utc),
                expires_at: expires.and_then(|e| DateTime::parse_from_rfc3339(&e).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
            };
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }
}
```

**Put 操作**：
```rust
async fn put(&self, namespace: &str, key: &str, value: Value, _index: Option<Vec<String>>)
    -> Result<(), StoreError>
{
    let now = Utc::now().to_rfc3339();
    let json_str = serde_json::to_string(&value)?;
    let expires_at = self.ttl_config.default_ttl.map(|ttl| {
        (Utc::now() + ttl).to_rfc3339()
    });

    sqlx::query(
        "INSERT INTO store_items (namespace, key, value, created_at, updated_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (namespace, key) DO UPDATE SET
            value = excluded.value,
            updated_at = excluded.updated_at,
            expires_at = excluded.expires_at"
    )
    .bind(namespace)
    .bind(key)
    .bind(&json_str)
    .bind(&now)
    .bind(&now)
    .bind(&expires_at)
    .execute(&self.pool)
    .await?;

    Ok(())
}
```

**Delete 操作**：
```rust
async fn delete(&self, namespace: &str, key: &str) -> Result<(), StoreError> {
    let result = sqlx::query("DELETE FROM store_items WHERE namespace = ? AND key = ?")
        .bind(namespace)
        .bind(key)
        .execute(&self.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound {
            namespace: namespace.to_string(),
            key: key.to_string(),
        });
    }

    Ok(())
}
```

#### 6.2.4 过滤表达式到 SQL 翻译

**Where 子句构建**：
```rust
impl SqliteStore {
    fn build_filter_query(&self, query: &SearchQuery) -> (String, Vec<Value>) {
        let mut conditions = vec!["namespace LIKE ? || '%'");
        let mut params = vec![json!(query.namespace_prefix)];

        if let Some(filter) = &query.filter {
            let (sql_clause, filter_params) = filter.to_sql();
            conditions.push(&sql_clause);
            params.extend(filter_params);
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT namespace, key, value, created_at, updated_at, expires_at
             FROM store_items WHERE {} LIMIT ? OFFSET ?",
            where_clause
        );

        params.push(json!(query.limit));
        params.push(json!(query.offset));

        (sql, params)
    }
}
```

#### 6.2.5 命名空间列举

**前缀/后缀过滤**：
```rust
async fn list_namespaces(&self, prefix: Option<&str>, suffix: Option<&str>,
                         max_depth: Option<usize>, limit: Option<usize>, offset: Option<usize>)
    -> Result<Vec<String>, StoreError>
{
    let mut query = String::from("SELECT DISTINCT namespace FROM store_items");
    let mut conditions = Vec::new();
    let mut params = Vec::new();

    if let Some(p) = prefix {
        conditions.push("namespace LIKE ?");
        params.push(format!("{}%", p));
    }

    if let Some(s) = suffix {
        if p.is_empty() {
            conditions.push("namespace LIKE ?");
        } else {
            conditions.push("namespace LIKE ?");
        }
        params.push(format!("%{}", s));
    }

    if !conditions.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(&conditions.join(" AND "));
    }

    if let Some(lim) = limit {
        query.push_str(&format!(" LIMIT {}", lim));
    }
    if let Some(off) = offset {
        query.push_str(&format!(" OFFSET {}", off));
    }

    let rows = sqlx::query_as::<_, (String,)>(&query)
        .bind_all(&params)
        .fetch_all(&self.pool)
        .await?;

    let namespaces: Vec<String> = rows.into_iter()
        .map(|(ns,)| ns)
        .collect();

    Ok(namespaces)
}
```

#### 6.2.6 批量操作

**事务支持**：
```rust
async fn batch(&self, ops: Vec<StoreOp>) -> Result<Vec<StoreResult>, StoreError> {
    let mut tx = self.pool.begin().await?;
    let mut results = Vec::new();

    for op in ops {
        let result = match op {
            StoreOp::Get { namespace, key } => {
                let item = Self::get_tx(&mut tx, &namespace, &key).await?;
                StoreResult::Item(item)
            }
            StoreOp::Put { namespace, key, value, index } => {
                Self::put_tx(&mut tx, &namespace, &key, value, index).await?;
                StoreResult::None
            }
            StoreOp::Delete { namespace, key } => {
                Self::delete_tx(&mut tx, &namespace, &key).await?;
                StoreResult::None
            }
            StoreOp::Search(query) => {
                let items = Self::search_tx(&mut tx, query).await?;
                StoreResult::Items(items)
            }
            StoreOp::ListNamespaces { prefix, suffix, max_depth, limit } => {
                let ns = Self::list_namespaces_tx(&mut tx, prefix, suffix, max_depth, limit).await?;
                StoreResult::Namespaces(ns)
            }
        };
        results.push(result);
    }

    tx.commit().await?;
    Ok(results)
}
```

#### 6.2.7 错误处理

**完整的错误映射**：
```rust
impl From<sqlx::Error> for StoreError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => StoreError::NotFound {
                namespace: String::from("unknown"),
                key: String::from("unknown"),
            },
            sqlx::Error::Database(db_err) => {
                if db_err.is_unique_violation() {
                    StoreError::Storage(format!("Unique constraint violation: {}", db_err))
                } else if db_err.is_foreign_key_violation() {
                    StoreError::Storage(format!("Foreign key violation: {}", db_err))
                } else {
                    StoreError::Storage(format!("Database error: {}", db_err))
                }
            }
            _ => StoreError::Storage(format!("SQL error: {}", err)),
        }
    }
}
```

#### 6.2.8 PostgreSQL 特定优化

**JSONB 路径查询**：
```rust
impl PostgresStore {
    fn json_extract_path(&self, path: &str) -> String {
        if path.contains('.') {
            // "metadata.status" -> "value->'metadata'->>'status'"
            let parts: Vec<&str> = path.split('.').collect();
            let mut expr = String::from("value");
            for (i, part) in parts.iter().enumerate() {
                if i == parts.len() - 1 {
                    expr.push_str(&format!("->>'{}'", part));
                } else {
                    expr.push_str(&format!("->'{}'", part));
                }
            }
            expr
        } else {
            format!("value->>'{}'", path)
        }
    }
}
```

**连接池配置**：
```rust
impl SqliteStore {
    pub async fn new(db_path: &str) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite:{}", db_path))
            .await?;

        // 运行迁移
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS store_items (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                expires_at TEXT,
                PRIMARY KEY (namespace, key)
            );
            CREATE INDEX IF NOT EXISTS idx_store_namespace ON store_items(namespace);
        "#)
        .execute(&pool)
        .await?;

        Ok(Self {
            pool,
            index_config: None,
            ttl_config: TTLConfig::default(),
        })
    }
}
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

impl Item {
    /// TTL 检查辅助方法
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}
```

### 9.3 过期清理逻辑

**MemoryStore 实现**：

```rust
impl MemoryStore {
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

**SQL 后端实现**：

```rust
impl SqliteStore {
    /// 清理过期项（SQLite）
    pub async fn sweep_expired_items(&self) -> Result<usize, StoreError> {
        let now = Utc::now().to_rfc3339();
        let limit = self.ttl_config.sweep_max_items;

        let result = sqlx::query(
            "DELETE FROM store_items
             WHERE expires_at IS NOT NULL
             AND expires_at < ?
             LIMIT ?"
        )
        .bind(&now)
        .bind(limit as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }
}

impl PostgresStore {
    /// 清理过期项（PostgreSQL）
    pub async fn sweep_expired_items(&self) -> Result<usize, StoreError> {
        let now = Utc::now();
        let limit = self.ttl_config.sweep_max_items;

        let result = sqlx::query(
            "DELETE FROM store_items
             WHERE expires_at IS NOT NULL
             AND expires_at < $1
             LIMIT $2"
        )
        .bind(now)
        .bind(limit as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as usize)
    }
}
```

### 9.4 惰性过期清理

**get() 操作中的过期检查**：

```rust
#[async_trait]
impl Store for MemoryStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Item>, StoreError> {
        let full_key = format!("{}:{}", namespace, key);

        // 惰性过期检查
        {
            let items = self.items.read().await;
            if let Some(item) = items.get(&full_key) {
                if item.is_expired() {
                    drop(items);
                    let mut items = self.items.write().await;
                    items.remove(&full_key);
                    return Ok(None);
                }
            }
        }

        let items = self.items.read().await;
        let item = items.get(&full_key).cloned();

        // refresh_on_read 逻辑
        if let (Some(mut item), true) = (item, self.ttl_config.refresh_on_read) {
            if self.ttl_config.default_ttl.is_some() {
                drop(items);
                let new_expires = Utc::now() + self.ttl_config.default_ttl.unwrap();
                let mut items = self.items.write().await;
                if let Some(existing) = items.get_mut(&full_key) {
                    existing.expires_at = Some(new_expires);
                    item.expires_at = Some(new_expires);
                }
                return Ok(Some(item));
            }
        }

        Ok(item)
    }
}
```

**search() 操作中的过期过滤**：

```rust
#[async_trait]
impl Store for MemoryStore {
    async fn search(&self, query: SearchQuery) -> Result<SearchResult, StoreError> {
        let items = self.items.read().await;

        let filtered: Vec<Item> = items.values()
            .filter(|item| {
                // 命名空间过滤
                if !item.namespace.starts_with(&query.namespace_prefix) {
                    return false;
                }
                // 惰性过期过滤
                if item.is_expired() {
                    return false;
                }
                // 表达式过滤
                if let Some(filter) = &query.filter {
                    return filter.matches(&item.value);
                }
                true
            })
            .cloned()
            .collect();

        Ok(SearchResult {
            total_count: filtered.len(),
            items: filtered.into_iter()
                .skip(query.offset)
                .take(query.limit)
                .map(|item| SearchItem { item, score: None })
                .collect(),
        })
    }
}
```

### 9.5 使用示例

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

// SQL 后端
let sqlite_store = SqliteStore::new("store.db").await?;
let pg_store = PostgresStore::new("postgresql://localhost/store").await?;

// 手动触发清理
let cleaned = sqlite_store.sweep_expired_items().await?;
println!("Cleaned {} expired items", cleaned);
```

### 9.6 测试覆盖

完整的 TTL 功能测试套件：

```rust
#[cfg(test)]
mod ttl_tests {
    use super::*;

    #[tokio::test]
    async fn test_default_ttl() {
        let mut store = MemoryStore::new();
        store.ttl_config.default_ttl = Some(Duration::from_millis(100));

        store.put("ns", "key1", json!({"data": "test"}), None).await.unwrap();

        // 立即获取应该成功
        let item = store.get("ns", "key1").await.unwrap();
        assert!(item.is_some());

        // 等待过期
        tokio::time::sleep(Duration::from_millis(150)).await;

        // 过期后应该返回 None
        let item = store.get("ns", "key1").await.unwrap();
        assert!(item.is_none());
    }

    #[tokio::test]
    async fn test_refresh_on_read() {
        let mut store = MemoryStore::new();
        store.ttl_config.default_ttl = Some(Duration::from_millis(100));
        store.ttl_config.refresh_on_read = true;

        store.put("ns", "key1", json!({"data": "test"}), None).await.unwrap();

        // 等待 50ms，读取刷新 TTL
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _item = store.get("ns", "key1").await.unwrap();

        // 再等 60ms，应该仍然存在（因为被刷新了）
        tokio::time::sleep(Duration::from_millis(60)).await;
        let item = store.get("ns", "key1").await.unwrap();
        assert!(item.is_some());
    }

    #[tokio::test]
    async fn test_sweep_expired_items() {
        let mut store = MemoryStore::new();
        store.ttl_config.default_ttl = Some(Duration::from_millis(50));
        store.ttl_config.sweep_max_items = 10;

        // 添加多个项
        for i in 0..20 {
            store.put("ns", &format!("key{}", i), json!({"data": i}), None).await.unwrap();
        }

        // 等待过期
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 清理应该只处理 sweep_max_items 个
        let cleaned = store.sweep_expired_items().await.unwrap();
        assert_eq!(cleaned, 10);
    }

    #[tokio::test]
    async fn test_lazy_expiration_in_search() {
        let mut store = MemoryStore::new();
        store.ttl_config.default_ttl = Some(Duration::from_millis(50));

        store.put("ns", "key1", json!({"data": "test1"}), None).await.unwrap();
        store.put("ns", "key2", json!({"data": "test2"}), None).await.unwrap();

        // 等待过期
        tokio::time::sleep(Duration::from_millis(100)).await;

        let results = store.search(SearchQuery {
            namespace_prefix: "ns".to_string(),
            ..Default::default()
        }).await.unwrap();

        // 过期项应该被过滤掉
        assert_eq!(results.items.len(), 0);
    }
}
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

