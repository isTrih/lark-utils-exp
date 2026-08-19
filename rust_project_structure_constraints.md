# Rust 中大型项目结构约束文档

## 1. 文档目的

本文档用于约束 Rust 中大型项目的目录结构、模块职责、配置管理、错误处理、日志、数据库访问、业务分层和扩展方式。

目标是让项目在持续迭代中保持：

- 结构清晰
- 模块边界明确
- 业务逻辑可测试
- 外部依赖可替换
- 配置集中管理
- 入口文件保持简洁
- 适合后续扩展为中大型后端、数据同步服务、CLI 工具或任务调度系统

---

## 2. 总体架构原则

项目应优先采用：

```text
单 crate + 清晰目录分层
```

不建议项目初期直接拆分为 workspace。只有当模块体量、团队规模、编译隔离、复用需求明显增加后，再考虑拆分为 workspace。

核心设计原则：

```text
main.rs           只负责启动
config.rs         只负责读取配置
infrastructure    负责连接外部系统
repository        只负责数据库访问
service           负责业务逻辑
api               只负责请求响应
domain            存放核心业务模型
job               存放定时任务 / 同步任务
dto               存放接口输入输出结构
```

---

## 3. 推荐目录结构

标准结构如下：

```text
my_app/
├── Cargo.toml
├── .env
├── .env.example
├── .gitignore
├── README.md
├── migrations/
│   └── 202606300001_create_tables.sql
│
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── config.rs
│   ├── error.rs
│   ├── state.rs
│   │
│   ├── app/
│   │   ├── mod.rs
│   │   └── bootstrap.rs
│   │
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── creator.rs
│   │   ├── video.rs
│   │   ├── live.rs
│   │   ├── task.rs
│   │   └── lark_sheet.rs
│   │
│   ├── repository/
│   │   ├── mod.rs
│   │   ├── creator_repo.rs
│   │   ├── video_repo.rs
│   │   ├── live_repo.rs
│   │   └── task_repo.rs
│   │
│   ├── service/
│   │   ├── mod.rs
│   │   ├── creator_service.rs
│   │   ├── video_service.rs
│   │   ├── live_service.rs
│   │   ├── task_service.rs
│   │   └── sync_service.rs
│   │
│   ├── infrastructure/
│   │   ├── mod.rs
│   │   ├── database.rs
│   │   ├── lark_client.rs
│   │   ├── lark_sheet_client.rs
│   │   └── http_client.rs
│   │
│   ├── api/
│   │   ├── mod.rs
│   │   ├── health.rs
│   │   ├── task_api.rs
│   │   └── sync_api.rs
│   │
│   ├── dto/
│   │   ├── mod.rs
│   │   ├── task_dto.rs
│   │   ├── video_dto.rs
│   │   └── live_dto.rs
│   │
│   ├── job/
│   │   ├── mod.rs
│   │   ├── pull_video_snapshot.rs
│   │   ├── pull_live_snapshot.rs
│   │   └── sync_lark_sheet.rs
│   │
│   └── utils/
│       ├── mod.rs
│       ├── time.rs
│       └── id.rs
│
├── tests/
│   ├── api_test.rs
│   └── service_test.rs
│
└── benches/
    └── sync_bench.rs
```

---

## 4. 模块职责约束

### 4.1 `main.rs`

`main.rs` 只负责程序启动，不允许堆叠业务逻辑。

推荐写法：

```rust
use my_app::app::bootstrap::bootstrap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap().await
}
```

禁止在 `main.rs` 中直接写：

- 复杂业务逻辑
- SQL 查询
- 飞书 API 调用
- HTTP handler
- 大量配置解析
- 定时任务实现细节

---

### 4.2 `lib.rs`

`lib.rs` 负责将模块挂载到 Rust 模块树中。

示例：

```rust
pub mod config;
pub mod error;
pub mod state;

pub mod app;
pub mod domain;
pub mod service;
pub mod repository;
pub mod infrastructure;
pub mod api;
pub mod dto;
pub mod job;
pub mod utils;
```

新增文件后必须确保其被正确挂载，否则会出现 rust-analyzer 的 `unlinked-file` 警告。

---

### 4.3 `config.rs`

`config.rs` 只负责读取配置。

所有环境变量读取必须集中在 `config.rs` 中完成，其他模块不得直接调用：

```rust
std::env::var(...)
```

推荐结构：

```rust
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub lark_app_id: String,
    pub lark_app_secret: String,
    pub lark_base_url: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            database_url: env::var("DATABASE_URL")?,
            lark_app_id: env::var("LARK_APP_ID")?,
            lark_app_secret: env::var("LARK_APP_SECRET")?,
            lark_base_url: env::var("LARK_BASE_URL")
                .unwrap_or_else(|_| "https://open.larksuite.com".to_string()),
        })
    }
}
```

约束：

- `.env` 用于本地开发
- `.env.example` 必须提交
- `.env` 不得提交到 Git
- 生产环境应通过系统环境变量、Docker env、CI Secret 或部署平台配置注入
- 业务模块不得自行读取环境变量

---

### 4.4 `state.rs`

`state.rs` 用于定义全局共享状态。

例如：

```rust
use crate::config::Config;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: PgPool,
}
```

适合放入：

- 配置对象
- 数据库连接池
- Redis 连接池
- HTTP client
- 飞书 client
- 业务 service 聚合对象

约束：

- `AppState` 不应包含临时请求数据
- `AppState` 不应承担业务逻辑
- `AppState` 应尽量保持可 clone，便于 Web 框架共享

---

### 4.5 `domain/`

`domain/` 存放核心业务模型。

该层应保持干净，原则上不依赖：

- HTTP 框架
- 数据库连接池
- 飞书 SDK
- Redis
- 第三方 API client

示例：

```rust
#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Pending,
    Running,
    Finished,
    Failed,
}
```

约束：

- 领域模型不应直接等同于数据库表结构
- 领域模型不应直接作为 HTTP 返回结构
- 领域模型应表达业务含义，而不是外部系统细节

---

### 4.6 `dto/`

`dto/` 存放接口输入输出结构。

示例：

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
}

#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub id: i64,
    pub title: String,
    pub status: String,
}
```

约束：

- HTTP 请求体使用 Request DTO
- HTTP 响应体使用 Response DTO
- 不要直接把数据库 record 暴露给前端
- 不要直接把领域模型暴露给外部接口

---

### 4.7 `repository/`

`repository/` 只负责数据库访问。

示例：

```rust
use sqlx::PgPool;

pub struct TaskRepository {
    db: PgPool,
}

impl TaskRepository {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<TaskRecord>> {
        let task = sqlx::query_as!(
            TaskRecord,
            r#"
            SELECT id, title, status
            FROM tasks
            WHERE id = $1
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(task)
    }
}
```

约束：

- repository 只写 SQL 或数据库 ORM 操作
- repository 不写业务规则
- repository 不处理 HTTP 请求
- repository 不直接调用外部 API
- repository 方法名应表达数据访问意图

---

### 4.8 `service/`

`service/` 存放业务逻辑，是项目最核心的组织层。

示例：

```rust
pub struct TaskService {
    task_repo: TaskRepository,
}

impl TaskService {
    pub fn new(task_repo: TaskRepository) -> Self {
        Self { task_repo }
    }

    pub async fn create_task(&self, title: String) -> anyhow::Result<()> {
        if title.trim().is_empty() {
            anyhow::bail!("任务标题不能为空");
        }

        self.task_repo.create(title).await?;

        Ok(())
    }
}
```

约束：

- 参数校验、业务规则、业务流程编排应放在 service
- service 可以调用 repository
- service 可以调用 infrastructure 中封装好的外部 client
- service 不直接处理 HTTP request / response
- service 不直接读取环境变量

---

### 4.9 `infrastructure/`

`infrastructure/` 存放外部系统连接与封装。

适合放入：

- 数据库连接初始化
- 飞书 client 初始化
- 飞书表格 API 封装
- Redis 初始化
- HTTP client 初始化
- 第三方平台 SDK 封装

示例：

```rust
use crate::config::Config;
use open_lark::prelude::*;

pub fn create_lark_client(config: &Config) -> anyhow::Result<Client> {
    let client = Client::builder()
        .app_id(&config.lark_app_id)
        .app_secret(&config.lark_app_secret)
        .base_url(&config.lark_base_url)
        .build()?;

    Ok(client)
}
```

约束：

- 外部 API 的 SDK 初始化不得散落在业务代码中
- 外部系统调用应尽量封装，避免 service 直接感知复杂 SDK 细节
- 密钥、地址、超时时间等配置必须来自 `Config`

---

### 4.10 `api/`

`api/` 存放 HTTP handler。

示例：

```rust
use axum::{extract::State, Json};
use crate::state::AppState;

pub async fn list_tasks(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok"
    }))
}
```

约束：

- api 层只负责请求解析、响应组装、状态码处理
- api 层不直接写 SQL
- api 层不堆业务逻辑
- api 层应调用 service 完成业务
- api 层使用 dto 作为输入输出结构

---

### 4.11 `job/`

`job/` 存放定时任务、同步任务、批处理任务。

适合放入：

- 飞书表格同步任务
- 视频数据拉取任务
- 直播数据拉取任务
- 历史数据补偿任务
- 清理任务
- 定时统计任务

约束：

- job 负责触发和编排任务
- 具体业务逻辑仍应放在 service
- job 不直接写复杂 SQL
- job 不直接读取环境变量

---

### 4.12 `utils/`

`utils/` 存放通用工具函数。

适合放入：

- 时间处理
- ID 生成
- 字符串处理
- 格式化工具
- 通用转换函数

约束：

- 不要把业务逻辑放入 utils
- utils 不应依赖 service、repository、api
- utils 中的函数应具备通用性

---

## 5. 环境变量约束

项目根目录必须提供 `.env.example`。

示例：

```env
DATABASE_URL=postgres://user:password@localhost:5432/app
LARK_APP_ID=
LARK_APP_SECRET=
LARK_BASE_URL=https://open.larksuite.com
RUST_LOG=info
```

`.gitignore` 必须包含：

```gitignore
.env
target/
```

约束：

- `.env` 只用于本地开发
- `.env.example` 用于说明必须配置哪些变量
- 生产环境不得依赖本地 `.env`
- 密钥不得硬编码在源码中
- 所有配置必须通过 `Config::from_env()` 统一读取

---

## 6. 错误处理约束

推荐依赖：

```bash
cargo add anyhow thiserror
```

应用层可以使用：

```rust
pub type AppResult<T> = anyhow::Result<T>;
```

领域错误可以使用 `thiserror`：

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,

    #[error("invalid input: {0}")]
    InvalidInput(String),
}
```

约束：

- 应用层可优先使用 `anyhow::Result<T>`
- 可预期、需要匹配处理的业务错误应使用 `thiserror`
- 不要在业务代码里大量 `unwrap()`
- 不要在业务代码里大量 `expect()`
- `unwrap()` 仅允许用于测试、样例代码或明确不可能失败的位置
- 对外接口应返回清晰、可理解的错误信息

---

## 7. 日志约束

推荐依赖：

```bash
cargo add tracing tracing-subscriber
```

启动初始化：

```rust
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();
}
```

业务中使用：

```rust
tracing::info!("start sync lark sheet");
tracing::warn!("missing optional field");
tracing::error!("sync failed: {:?}", err);
```

约束：

- 不要用大量 `println!` 作为正式日志
- 正式日志统一使用 `tracing`
- 日志级别应合理区分 `debug`、`info`、`warn`、`error`
- 生产环境日志级别通过 `RUST_LOG` 控制
- 日志中不得输出 app_secret、token、password 等敏感信息

---

## 8. 数据库访问约束

数据库连接应封装在 `infrastructure/database.rs`。

示例：

```rust
use sqlx::{PgPool, postgres::PgPoolOptions};

pub async fn create_pool(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    Ok(pool)
}
```

约束：

- 所有 SQL 访问通过 repository 完成
- 不允许在 api 层直接写 SQL
- 不允许在 main.rs 中直接写 SQL
- migration 文件统一放在 `migrations/`
- 数据库连接池统一放入 `AppState`
- 大整数 ID 应根据业务情况使用 `i64`、`String` 或 Decimal，不得随意使用 `i32`

---

## 9. 启动流程约束

启动流程应集中在 `app/bootstrap.rs`。

示例：

```rust
use crate::config::Config;
use crate::infrastructure::database::create_pool;
use crate::state::AppState;

pub async fn bootstrap() -> anyhow::Result<()> {
    let config = Config::from_env()?;

    let db = create_pool(&config.database_url).await?;

    let state = AppState {
        config,
        db,
    };

    // 初始化 router / job / server
    // start_server(state).await?;

    Ok(())
}
```

约束：

- main.rs 只调用 bootstrap
- bootstrap 负责初始化配置、日志、数据库、外部 client、路由、任务
- bootstrap 不应包含具体业务逻辑
- 具体业务逻辑应下沉到 service

---

## 10. Cargo.toml 建议

基础依赖建议：

```toml
[package]
name = "my_app"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1"
thiserror = "2"
dotenvy = "0.15"

serde = { version = "1", features = ["derive"] }
serde_json = "1"

tokio = { version = "1", features = ["full"] }

tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "macros", "chrono", "uuid"] }
```

如果使用 Web 框架，可补充：

```toml
axum = "0.8"
tower-http = { version = "0.6", features = ["cors", "trace"] }
```

约束：

- 不要无理由引入重型依赖
- 新增依赖前应确认是否真的需要
- 对于没有发布 crates.io 新版本的依赖，优先锁定 git commit
- 不建议长期依赖未锁定的 git branch

Git 依赖推荐写法：

```toml
some_crate = { git = "https://github.com/owner/repo.git", rev = "具体_commit_hash" }
```

---

## 11. 模块声明约束

每个目录必须有 `mod.rs`。

示例：

```text
src/service/
├── mod.rs
├── task_service.rs
└── sync_service.rs
```

`src/service/mod.rs`：

```rust
pub mod task_service;
pub mod sync_service;
```

如果某个模块只在当前 crate 内部使用，可以使用：

```rust
pub(crate) mod task_service;
```

约束：

- 新建 `.rs` 文件后必须在上级模块中声明
- 对外暴露使用 `pub`
- crate 内部使用优先 `pub(crate)`
- 不需要暴露的函数、结构体、字段不要随意加 `pub`

---

## 12. 分层调用方向约束

推荐调用方向：

```text
api -> service -> repository -> database
api -> service -> infrastructure -> external system
job -> service -> repository / infrastructure
```

禁止反向依赖：

```text
repository -> service
domain -> api
domain -> infrastructure
config -> service
utils -> service
```

模块职责边界必须保持稳定。

---

## 13. 测试约束

推荐测试目录：

```text
tests/
├── api_test.rs
└── service_test.rs
```

约束：

- 核心 service 应尽量可单元测试
- repository 可使用测试数据库进行集成测试
- domain 层逻辑应优先写单元测试
- 不要让测试依赖真实生产环境变量
- 测试配置应使用独立 `.env.test` 或显式传参

---

## 14. 何时升级为 Workspace

初期不建议拆 workspace。

当出现以下情况时，可以考虑拆分：

- domain 需要被多个应用复用
- CLI、Web 服务、定时任务需要独立发布
- infrastructure 依赖过重，影响编译速度
- 团队协作需要更强边界
- 单 crate 文件数量明显过多，维护成本上升

workspace 示例：

```text
my_workspace/
├── Cargo.toml
├── crates/
│   ├── app/
│   ├── domain/
│   ├── infrastructure/
│   ├── migration/
│   └── cli/
```

根 `Cargo.toml`：

```toml
[workspace]
members = [
    "crates/app",
    "crates/domain",
    "crates/infrastructure",
    "crates/cli",
]
resolver = "2"
```

约束：

- 不要过早 workspace 化
- workspace 会增加依赖管理、可见性管理和编译组织成本
- 只有当项目复杂度确实上升后再拆分

---

## 15. 推荐起步结构

项目初期不需要一次性创建所有目录，可以先从最小结构开始：

```text
src/
├── main.rs
├── lib.rs
├── config.rs
├── state.rs
├── app/
│   ├── mod.rs
│   └── bootstrap.rs
└── infrastructure/
    ├── mod.rs
    └── lark_client.rs
```

随着业务增加，再逐步补充：

```text
service/
repository/
api/
dto/
job/
domain/
utils/
```

---

## 16. 最终约束总结

必须遵守：

```text
1. main.rs 只负责启动
2. config.rs 统一读取环境变量
3. .env 不提交，.env.example 必须提交
4. AppState 统一管理共享状态
5. repository 只操作数据库
6. service 只处理业务逻辑
7. api 只处理请求响应
8. infrastructure 只封装外部系统
9. job 只负责任务触发和流程编排
10. domain 不依赖数据库、HTTP 框架和外部 SDK
11. dto 不等于数据库模型
12. 不要在业务代码中滥用 unwrap / expect
13. 正式日志统一使用 tracing
14. 新建文件必须挂到模块树
15. 不要过早拆 workspace
```

项目演进优先级：

```text
先保证结构清晰
再保证边界稳定
再做抽象复用
最后才做 workspace 拆分
```
