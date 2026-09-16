# Lark Utils Exp 项目长期约束

此文件是本项目给后续 AI/Codex 的长期记忆。修改代码前先阅读，并以当前用户的最新明确指令为最高优先级。不要把已经被纠正的历史假设重新引入代码。

## 工程原则

- 优先复用现有 Rust、Salvo、SQLx、OpenLark 和 Bun 结构，保持实现简单直接。
- 修改前先阅读现有实现和 migration；不要为局部需求创建不必要的抽象或旁路流程。
- PostgreSQL 是当前数据库。已经执行过的 migration 永远不能修改；数据库结构变化必须新增更高版本 migration，并同时提供 up/down。
- 不提交 Cookie、CSRF Token、飞书密钥、数据库连接串或真实登录态。
- 不要提交未跟踪的 `browser-extensions/xingtu-session-uploader.zip`，除非用户明确要求。

## 版本与发布

- `Cargo.toml` 的 `package.version` 是基础版本的唯一来源，必须遵循 SemVer。
- 不兼容变更升级 major，向后兼容的新功能升级 minor，向后兼容的问题修复升级 patch。
- Docker 正式发布前必须先完成 Git commit；commit message 要包含完整变更日志。
- Docker 必须通过 `bun scripts/docker-release.ts` 发布，不再手工使用日期版本标签。
- 每次 Docker 构建自动生成唯一 SemVer build metadata，包含 UTC 构建时间和 Git commit。
- 发布脚本应同时推送 `latest`、基础 SemVer 和不可变构建标签，并在完成后核对 digest。
- 程序启动日志、`GET /health`、OpenAPI 和 `/meta.json` 必须展示同一个完整构建版本。
- 发布前至少执行：`cargo fmt --all -- --check`、`cargo check`、`cargo test`、`git diff --check`。

## 直播与视频指标口径

- 星图直播的业务“场观PV”只使用 `live_exposure_pv`，来源字段是 `直播曝光pv`。
- 不要重新引入运行时 `viewer_pv`，也不要把 `cumulative_viewer_count` 猜成场观PV。
- 日报/项目卡片中的视频播放量和直播 PV 展示值除以 `10000`，保留两位小数；CPM 也保留两位小数。
- 平均 ACU 是当日直播场次的平均 ACU。
- “每日视频最终播放”是当天发布稿件当前最终播放量之和，会随后续同步变化。
- “每日新增播放”是当日全部视频最新总播放量与昨日快照的差值。
- 手动登记视频有播放量时保存真实值，缺失时保存 `0`；同一稿件同时存在星图和手动登记数据时，星图数据优先。

## 工作流与时间

- 所有业务日期和内置调度按 `Asia/Shanghai` 计算，不依赖容器系统默认时区。
- `periodic` 固定在北京时间 `12:00`、`18:00`；`morning` 在 `09:00`；`night` 在 `23:59`。不再提供按小时固定间隔配置。
- 自动工作流只检索启用且处于追踪窗口内的活动。项目之间按 `WORKFLOW_PROJECT_CONCURRENCY` 受控并行（默认 3，范围 1-16），单个项目内部步骤保持串行；一个项目失败不得阻断其他项目，全部完成后汇总为成功、部分失败或全部失败。
- 单项目完整工作流受 `WORKFLOW_PROJECT_TIMEOUT_SECONDS` 限制（默认 3600 秒，范围 300-21600）；超时必须标记当前步骤失败、释放并发槽并继续后续项目。HTTP 请求断开不得取消已启动的完整工作流。
- 单项目可恢复错误按 `WORKFLOW_PROJECT_MAX_ATTEMPTS` 自动重试（默认 3，范围 1-5），由 `WORKFLOW_PROJECT_RETRY_DELAY_SECONDS`（默认 15 秒，范围 1-300）指数退避；登录态、权限和确定性配置错误禁止盲目重试。服务必须巡检并回收异常退出遗留的孤儿工作流，再自动执行周期同步补偿。
- 跨实例工作流事务锁执行期间必须定期发送数据库心跳，防止数据库中间层或连接生命周期导致 advisory lock 失联；锁心跳失败时必须立即停止工作流，禁止在失去全局锁后继续执行。
- 工作流接口尽量支持可选 `activity_period_id`，用于只执行指定项目。
- `tracking_end_date=YYYY-MM-DD` 时，北京时间结束日 `23:59` 执行最后一次更新，次日不再拉取。
- 星图导出长任务之间保留 `0..15` 秒随机间隔，避免集中请求。
- `night` 会从审核表同步审核结果到数据库。
- 每个期次允许只配置直播或只配置视频；关闭 `morning_workflow_enabled`/`morning_review_enabled` 只关闭审核通知，不得停止该期次的数据同步；API 同时接受 `audit_notice_enabled` 输入别名。
- `audit_table_id` 可省略、传 JSON `null` 或空字符串，入库统一为 SQL `NULL`；此时仍同步业务主表，只跳过审核表写入、审核结果回写和该内容的审核通知统计。禁止把字符串 `"NULL"` 当成表 ID。
- 已移除旧的 night `daily_video_table_id` 同步步骤；不要恢复。数据同步插件中的日报统计字段不属于这个旧步骤。
- `manual-sync` 只处理直播/视频手动登记多维表到数据库及业务多维表，不请求星图导出、不读取星图 Sheet、不导入 pending 来源、不发送审核通知。

## 通知规则

- 审核通知按项目分别发送，每个项目一张卡；审核人只读取该项目 `is_active=true` 的 auditor。
- 账号表中的 `ops_ids` 只用于星图登录态和工作流错误通知，不用于审核通知。
- 项目工作流错误必须携带绑定的星图账号 ID，只通知对应项目。
- 单项目重试耗尽或确定性失败后必须立即发送对应项目错误通知，禁止等待整批项目完成后才通知；否则其他项目卡死或进程退出会吞掉已确认的失败告警。
- 无法识别具体项目的兜底错误通知按 `receive_id_type + receive_id` 去重，不能因多个账号共用群聊而重复发送。
- 星图外部错误要保留服务端返回的中文原因；长任务 `status=4` 中的嵌套 `result.data` 也要进入错误详情。
- 错误、审核、日报卡片发送成功后必须写入 `card_message_history`，用于历史查询和撤回。
- 卡片历史落库失败只记录错误，不能把已经发送成功的卡片当作业务失败重试，避免重复通知。
- 消息历史只保存摘要和接收者元数据，不保存 Cookie、CSRF、完整卡片原文等敏感内容。

## 飞书接口兼容

- 飞书字段列表使用宽松原始 JSON 解析，只依赖 `data.items[].field_name`。
- 不要改回严格的 `ListFieldRequest` 字段模型；真实响应可能缺少 SDK 要求的 `is_hidden`，会造成假 warning。
- 字段预检拿到清单时应准确报告 `missing_fields`；元数据接口暂时不可用时只记录告警并继续原批量写入，诊断逻辑不能阻断业务。
- 批量写入失败必须记录 `table_id`、批次和该批全部写入字段，便于定位 `FieldNameNotFound`。

## HTTP 与运行环境

- CORS 由 `CORS_DOMAIN` 配置；主域与 `*.子域` 必须分别声明，只接受 `http/https` Origin，未配置时禁用跨域响应，禁止全开放 `*`。
- 服务和飞书数据同步插件都使用根路径，不再支持或恢复 `/auto` 反向代理前缀。
- 持久化日志目录由 `LOG_DIR` 控制；Docker 默认 `/app/logs`，生产环境必须挂载持久卷。
- 控制台与 JSONL 持久化日志时间戳统一使用北京时间，并显式包含 `+08:00` 时区偏移。
- `RUST_LOG=debug` 会显示 OpenLark、Hyper 和 HTTP/2 的底层 DEBUG 日志；`GoAway(NO_ERROR)` 是正常连接关闭，不应当作业务错误。
