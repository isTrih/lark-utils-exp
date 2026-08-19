# HTTP API 说明

本文档描述当前 Salvo 服务暴露的 HTTP 接口。示例默认服务地址为：

```text
https://api.example.com
```

## 通用约定

所有请求和响应默认使用 JSON，除非接口没有请求体。

成功响应大体分两类：

- 包装响应：`{ "ok": true, ... }`
- 查询列表：直接返回 JSON 数组

错误响应统一为：

```json
{
  "ok": false,
  "code": "bad_request",
  "message": "错误信息",
  "request_id": "8a2c..."
}
```

服务会接受或生成 `X-Request-ID`，在响应头和错误体中回传。内部错误只向客户端返回
`internal_error + 服务内部错误`，完整错误链仅进入结构化日志。除健康检查、文档、只读查询和
飞书连接器外，写接口统一要求 `Authorization: Bearer <MUTATION_API_TOKEN>`；未配置时兼容
回退到 `ADMIN_API_TOKEN`。唯一例外是 `POST /api/v1/xingtu/sessions`：内部浏览器插件可使用
共享的 `XINGTU_SESSION_UPLOAD_TOKEN`，管理员 Token 也继续兼容。上传专用 Token 不能访问
工作流、账号检查或管理接口。生产环境必须配置长随机 Token。

分页查询参数：

| 参数 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `limit` | integer | `50` | 返回条数，服务端限制为 `1..500` |
| `offset` | integer | `0` | 偏移量，小于 0 时按 0 处理 |

时间说明：

- 业务日期、调度时间、`stat_date` 均按 `Asia/Shanghai` 计算。
- `TIMESTAMPTZ` 字段是绝对时间点；服务端数据库连接会话设置为 `Asia/Shanghai`。

跨域访问：

- 服务使用 Salvo CORS 中间件处理普通请求和 `OPTIONS` 预检请求。
- 允许域名由 `CORS_DOMAIN` 环境变量配置，只接受 `http/https` Origin。
- `example.com` 只匹配主域，`*.example.com` 只匹配任意层级子域；需要同时允许二者时必须分别声明。
- 支持 JSON 数组（推荐）、方括号逗号列表和纯逗号列表，例如 `CORS_DOMAIN=["example.com","*.example.com"]`。
- 未配置 `CORS_DOMAIN` 时不返回跨域许可；空值、协议、端口、路径、格式错误或全开放 `*` 会导致服务拒绝启动。
- 响应中的 `Access-Control-Allow-Origin` 会回显实际请求 Origin，不会使用全局 `*`。
- 预检允许 `GET`、`POST`、`PUT`、`PATCH`、`DELETE`、`OPTIONS`，并回显浏览器申请的请求头。

## 响应压缩与可选数据保护

服务支持通过标准 `Accept-Encoding` 协商 `gzip`、`br` 和 `zstd` HTTP 响应压缩。生产访问仍必须使用 HTTPS；压缩只节省带宽，不是安全机制。

受信任服务端客户端调用 `/api/v1/**` 时，可以显式发送 `X-Data-Protection: aes-256-gcm+zstd`，请求将原 JSON 响应先 zstd 压缩、再用 AES-256-GCM 加密并封装。不发该头时响应保持完全兼容。飞书 `/api/data-sync/*` 协议不使用应用层信封。

密钥配置、信封格式、curl 与 Bun 解密脚本见 [API 数据保护与解密](./data-protection.md)。

## OpenAPI 文档

服务启动后会自动暴露 Salvo 官方 OpenAPI 文档：

| 地址 | 说明 |
| --- | --- |
| `GET /api-doc/openapi.json` | OpenAPI JSON |
| `GET /swagger-ui` | Swagger UI 页面 |

## 飞书多维表格数据同步插件

该协议用于飞书多维表格“数据同步插件”，不属于普通业务 API，因此响应格式遵循飞书连接器协议，而不是项目的 `{ "ok": true }` 包装格式。

### 接口列表

| 方法 | 地址 | 说明 |
| --- | --- | --- |
| `GET` | `/meta.json` | 返回数据同步插件元信息、配置页和数据接口地址 |
| `GET` | `/data-sync/config` | 项目选择配置页 |
| `POST` | `/api/data-sync/table-meta` | 返回日报表名和 15 个字段 |
| `POST` | `/api/data-sync/records` | 按飞书 `maxPageSize/pageToken` 返回日报记录 |

配置页读取 `GET /api/v1/queries/projects`，展示当前全部启用项目。用户选择后，配置页通过飞书官方 SDK 保存：

```json
{
  "activity_period_id": 1
}
```

数据同步插件固定部署在服务根路径：配置页为 `/data-sync/config`，静态资源和
项目列表分别使用 `/data-sync/config/*` 与 `/api/v1/queries/projects`。
生产环境应显式设置 `DATA_SYNC_PUBLIC_BASE_URL`，例如 `https://api.example.com`，
此时 `/meta.json` 返回的配置页是 `https://api.example.com/data-sync/config`。
该变量只填写协议、域名和可选端口，不包含路径。连接器地址始终规范化为 HTTPS；
即使后端收到 HTTP 请求，也只使用 `Forwarded` 或 `X-Forwarded-Host` 确定外部
Host，不会返回 HTTP 配置页地址。

`table-meta` 与 `records` 接收飞书协议请求，其中 `params` 和 `datasourceConfig` 均为 JSON 字符串。`records` 的分页 token 格式为 `offset_N`，每条记录使用 `period_{activity_period_id}_{YYYYMMDD}` 作为稳定主键。每次同步返回项目 `task_month` 所在自然月的全部日期，飞书负责依据主键完成新增、更新和删除。

### 日报字段口径

| 字段 | 统计口径 |
| --- | --- |
| 日期 | 项目 `task_month` 所在自然月的日期 |
| 星期 | 日期对应的中文星期 |
| 活动周期 | 从 `task_month` 起算的第 N 天 |
| 每日新增活跃作者 | 首次发布稿件日期为当天的去重作者数；优先使用作者 UID，缺失时使用作者名 |
| 累计活跃作者 | 截至当天发布过稿件的去重作者数，包含项目月份前已出现的作者 |
| 每日新增视频 | 发布日期为当天的稿件数 |
| 累计视频 | 截至当天已发布的稿件数 |
| 每日视频最终播放 | 当天发布稿件的当前最新播放量总和；后续同步会随最新数据更新 |
| 每日新增播放量 | 当天所有视频播放总量减去昨日所有视频播放总量 |
| 累计播放量 | 截至当天，每个视频取不晚于当天的最新快照后求和 |
| 每日新增主播 | 首次开播日期为当天的去重主播数；优先使用主播 UID，缺失时使用主播名 |
| 累计主播数 | 截至当天已开播的去重主播数，包含项目月份前已出现的主播 |
| 每日新增观看人次 | 当天直播场次的 `live_exposure_pv` 总和 |
| 累计观看人次 | 截至当天直播场次的 `live_exposure_pv` 总和 |
| 平均ACU | 当天直播场次 ACU 的算术平均值，保留两位小数；当天没有直播场次时为空 |

### 请求签名

配置 `DATA_SYNC_SECRET_KEY` 后，`table-meta` 与 `records` 会校验飞书请求签名：

```text
SHA1(timestamp + nonce + secretKey + 原始请求体)
```

对应请求头为 `X-Base-Request-Timestamp`、`X-Base-Request-Nonce` 和 `X-Base-Signature`。
服务只接受与当前时间相差不超过 5 分钟的 Unix 秒级 timestamp，并将 nonce 保存 10 分钟；
重复 nonce 会被拒绝，从而阻止合法请求被重放。校验失败时按照飞书协议返回错误码 `1254403`。
未配置 `DATA_SYNC_SECRET_KEY` 时允许无签名调用，仅适合本地调试。

## 健康检查

- `GET /live`：进程存活，不访问数据库或外部服务。
- `GET /ready`：数据库和关键内部结构就绪。
- `GET /health`：数据库检查与完整构建版本。
- 业务任务与账号状态见受保护的 `GET /api/v1/admin/status`。

### `GET /health`

检查服务和数据库连接是否正常。

请求示例：

```bash
curl "https://api.example.com/health"
```

响应示例：

```json
{
  "ok": true,
  "status": "healthy",
  "version": "0.2.0+build.20260806T010203Z.git.fa2eb7ca7c9e",
  "git_commit": "fa2eb7ca7c9e",
  "built_at": "2026-08-06T01:02:03.000Z"
}
```

`version` 是当前二进制的完整 SemVer；Docker 发布版会在基础版本后附加构建时间和
Git 提交。`git_commit` 与 `built_at` 同时写入镜像 OCI labels 和程序启动日志。

## 星图登录态

### `POST /api/v1/xingtu/sessions`

写入或更新某个项目星图账号的登录态。浏览器插件会调用这个接口。
登录态会以 AES-256-GCM 信封密文落库，账号 ID 作为附加认证数据；数据库不再保存新的
Cookie/CSRF 明文。部署必须配置 `XINGTU_SESSION_ENCRYPTION_KEY`，或显式复用
`API_DATA_ENCRYPTION_KEY`。

请求体：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `xingtu_account_id` | string | 是 | 项目星图账号 ID，例如 `demo-xingtu-account` |
| `cookie` | string | 是 | `xingtu.cn` 登录 Cookie，形如 `a=1; b=2` |
| `csrf_token` | string | 是 | 星图 CSRF Token |
| `session_key` | string 或 null | 否 | 星图页面/存储中可选的 session key |
| `user_agent` | string 或 null | 否 | 浏览器 User-Agent |
| `extra_headers` | object | 否 | 额外请求头，默认 `{}` |

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/xingtu/sessions" \
  -H "Authorization: Bearer $XINGTU_SESSION_UPLOAD_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "xingtu_account_id": "demo-xingtu-account",
    "cookie": "passport_auth_status_ss=...; passport_csrf_token=...",
    "csrf_token": "...",
    "session_key": "...",
    "user_agent": "Mozilla/5.0 ...",
    "extra_headers": {}
  }'
```

响应示例：

```json
{
  "ok": true
}
```

### `GET /api/v1/xingtu/sessions/{account_id}/check`

检查指定星图账号登录态是否有效。

路径参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `account_id` | string | 星图账号 ID |

请求示例：

```bash
curl "https://api.example.com/api/v1/xingtu/sessions/demo-xingtu-account/check"
```

响应示例：

```json
{
  "ok": true,
  "data": {
    "xingtu_account_id": "demo-xingtu-account",
    "project": "ROK",
    "valid": true,
    "notice_sent": false,
    "status_message": "success"
  }
```

字段说明：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `valid` | boolean | 登录态是否可用 |
| `notice_sent` | boolean | 登录态失效时是否已发送运维通知 |
| `status_message` | string 或 null | 星图接口返回或错误摘要 |

### `POST /api/v1/xingtu/sessions/check-all`

检查所有启用巡检的星图账号登录态。

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/xingtu/sessions/check-all"
```

响应示例：

```json
{
  "ok": true,
  "data": [
    {
      "xingtu_account_id": "demo-xingtu-account",
      "project": "ROK",
      "valid": true,
      "notice_sent": false,
      "status_message": "success"
    }
  ]
}
```

### 星图及工作流错误通知

登录态巡检发现失效，或 `morning`、`periodic`、`night` 完整工作流任一阶段执行失败时，服务会使用项目账号配置的通知模板和接收群发送错误卡片。

卡片变量：

| 变量 | 说明 |
| --- | --- |
| `ops_ids` | 项目账号配置的运维人员 ID，逗号分隔 |
| `project_name` | 项目账号对应的项目名称 |
| `error_type` | `星图登陆状态失效`、`星图数据拉取失败` 或 `工作流执行失败` |
| `error_detail` | 错误摘要、星图原始中文原因、账号 ID、工作流类型和北京时间 |

登录态失效详情示例：

```text
星图登陆态失效，请及时更新星图登陆态。
账号ID:demo-xingtu-account
错误信息:未登录
2026:07:30 12:00:00
```

星图导出接口、长任务或网络请求失败时，服务优先分类为 `星图数据拉取失败`。例如长任务 `status = 4` 中嵌套的 `result.data = "文档创建失败"` 会保留在 `error_detail`，不会只返回笼统的内部错误。每个项目工作流的错误上下文都会包含该期次绑定的账号 ID，因此只通知对应项目。若工作流在确定项目范围前失败、无法识别具体账号，服务会向启用账号的通知目标发送告警，并按 `receive_id_type + receive_id` 去重，避免多个账号配置到同一群时重复发送。

## 工作流

### `POST /api/v1/workflows/{kind}/run`

手动执行工作流。

工作流先检索仍处于追踪窗口内的启用活动期次，再按 `task_month` 和
`activity_period_id` 顺序串行执行。每个项目完整完成“星图拉取 -> pending 来源导入 ->
数据库及飞书表同步 -> 夜间审核结果回写或早间审核通知”后，才开始下一个项目。

路径参数：

| 参数 | 可选值 | 说明 |
| --- | --- | --- |
| `kind` | `morning` | 每日早审核工作流：拉取、导入、同步、通知审核 |
| `kind` | `periodic` | 周期同步工作流：拉取、导入、同步，不通知审核 |
| `kind` | `night` | 每日晚最终同步工作流：拉取、导入、同步并回写审核结果；不通知审核，数据标记为每日最终 |

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/workflows/morning/run"
```

所有工作流接口都支持可选的项目范围。不传请求体时执行全部符合条件的项目；传入
`activity_period_id` 时只处理该项目：

```bash
curl -X POST "https://api.example.com/api/v1/workflows/morning/run" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{ "activity_period_id": 2 }'
```

响应示例：

```json
{
  "ok": true,
  "data": {
    "workflow_run_id": 302,
    "workflow_kind": "morning",
    "processed_activity_period_ids": [2],
    "trace": {
      "traced_count": 2,
      "skipped_count": 0,
      "results": [
        {
          "content_config_id": 1,
          "feishu_source_id": 101,
          "project": "ROK",
          "period": "2026年7月第十四期",
          "xingtu_account_id": "demo-xingtu-account",
          "content_type": "video",
          "xingtu_task_id": "demo-video-task-id",
          "spreadsheet_url": "https://example.larksuite.com/sheets/ExampleSpreadsheetToken",
          "stat_date": "2026-07-05",
          "is_daily_final": false,
          "changed": true,
          "ticket_id": "xxx",
          "polls": 3
        }
      ]
    },
    "pending_import": {
      "discovered_sources": 1,
      "imported_sources": 1,
      "partial_sources": 0,
      "failed_sources": 0,
      "dead_lettered_sources": 0,
      "persisted_rows": 2660,
      "quarantined_rows": 0,
      "failures": []
    },
    "synced_activities": 1,
    "audit_result_sync": null,
    "audit_notice_sent": true
  }
}
```

返回字段说明：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `workflow_run_id` | integer | 可在管理接口中关联运行与步骤台账 |
| `workflow_kind` | string | 实际执行的工作流类型 |
| `processed_activity_period_ids` | integer[] | 本轮按照顺序完整执行成功的活动期次 ID |
| `trace.traced_count` | integer | 本轮尝试拉取的内容配置数量 |
| `trace.skipped_count` | integer | 本轮跳过数量 |
| `trace.results` | array | 每个内容配置的星图导出结果 |
| `pending_import` | object | 本轮来源发现、成功、部分、失败、死信、入库和隔离汇总 |
| `synced_activities` | integer | 本轮同步到多维表的活动配置数量 |
| `audit_result_sync` | object/null | `night` 工作流的审核结果回写汇总；其他工作流为 `null` |
| `audit_notice_sent` | boolean | 是否发送了审核通知 |

Night 工作流可以在同一业务日期重复执行。同一 `content_config_id + stat_date`
已有每日最终飞书来源时，服务会复用原 `feishu_source_id`，更新为本次导出的链接，
并将来源重置为 `pending` 后重新导入，不会触发
`uq_feishu_source_daily_final` 唯一约束错误。

同步主表和审核表前会尽力读取目标多维表字段清单。如果字段清单可用且来源数据包含目标表
不存在的字段，工作流会在批量写入前终止，错误链包含 `table_id`、`missing_fields`、
`write_fields` 和 `table_fields`。字段清单按宽松 JSON 结构读取，仅要求 `field_name`，不会因
SDK 字段模型中的其它可选属性缺失而误判失败。如果飞书字段列表接口返回空 `data` 或暂时
不可用，字段预检会记录包含 `table_id`、操作类型和全部写入字段的告警，然后继续执行原有
批量写入，不会让诊断步骤阻断业务同步。若飞书在写入阶段返回 `FieldNameNotFound`，错误中
仍会包含失败批次的完整字段列表，避免只看到错误码却无法判断缺少哪一列。

### `POST /api/v1/workflows/manual-sync/run`

只同步当前启用活动中已配置的直播、视频手动登记表。该接口不会请求星图导出、不会更新
`source_spreadsheet_url`、不会读取任何星图 Sheet、不会导入 pending 星图来源，也不会发送审核通知。

手动登记数据的完整业务字段会按现有规则写入数据库和业务主表；同步到 audit 表时只保留业务
唯一键，并在启用手动数据自动审核时写入审核结果。手动登记表或业务主表中供其他业务展示的
额外字段会直接丢弃，不会写入 audit 表。主表中同一唯一键已经标记为 `星图数据` 时会跳过该
手动记录，继续保持星图数据优先。

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/workflows/manual-sync/run"
```

指定项目时传入：`{ "activity_period_id": 2 }`。该专项接口是人工修复入口，不受
`tracking_end_date` 限制，但项目仍须处于启用状态。

响应示例：

```json
{
  "ok": true,
  "data": {
    "synced_activities": 1,
    "live_configs": 1,
    "video_configs": 1
  }
}
```

### `POST /api/v1/workflows/audit/run`

只检查当前 audit 表待审核数量并发送审核通知，不执行星图拉取、pending 导入或同步流程。
审核卡片的 `project_name` 模板变量取自当前项目配置的 `project`。不同活动期次独立统计、
独立发送，一张卡片不会再混入其他项目；审核人也分别读取所属项目当前启用的 auditor。

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/workflows/audit/run"
```

可选请求体：`{ "activity_period_id": 2 }`。显式指定项目时可人工检查已过追踪窗口但仍启用
的历史项目；不指定时只处理当前追踪窗口内的项目。

响应示例：

```json
{
  "ok": true,
  "data": {
    "audit_notice_sent": true,
    "pending_items": [
      {
        "project_name": "ROK",
        "period": "2026年7月第十四期",
        "video_nums": "12",
        "live_nums": "3",
        "audit_table_url": "[2026年7月第十四期](https://example.feishu.cn/base/xxx)"
      }
    ]
  }
}
```

### `POST /api/v1/workflows/audit-results/sync`

只读取当前启用活动的直播/视频审核表，将已经填写的非空审核结果回写到数据库。视频审核表还会同时读取文本字段 `审核标签`，回写为视频的 `label`。不会执行星图拉取、Sheet 导入、飞书主表同步或审核通知。

视频按 `视频/图文ID` 更新 `video_content.audit_result` 和 `video_content.label`，直播按 `直播间ID` 更新 `live_session.audit_result`。空白审核结果保持数据库中的审核结果和标签原值，避免将待审核记录误清空；审核结果非空时，空白或缺失的 `审核标签` 会将 `label` 清为 `null`。

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/workflows/audit-results/sync"
```

可选请求体：`{ "activity_period_id": 2 }`。

响应示例：

```json
{
  "ok": true,
  "data": {
    "tables_processed": 4,
    "records_read": 320,
    "reviewed_records": 128,
    "database_rows_updated": 12
  }
}
```

`reviewed_records` 是审核表中已填写结果且唯一键有效的记录数；`database_rows_updated` 只统计数据库中实际发生变化的行数，因此重复执行是幂等的。

### `POST /api/v1/workflows/pending/import`

只导入数据库中已有的未导入飞书来源。适合补偿“已经拿到 Sheet 链接，但还没有入库”的数据。

请求体：

| 字段 | 类型 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- | --- |
| `limit` | integer | 否 | `200` | 最多处理多少条来源 |
| `activity_period_id` | integer | 否 | 全部 | 只补偿指定活动期次的来源 |

请求示例：

```bash
curl -X POST "https://api.example.com/api/v1/workflows/pending/import" \
  -H "Content-Type: application/json" \
  -d '{ "limit": 200, "activity_period_id": 2 }'
```

响应示例：

```json
{
  "ok": true,
  "data": {
    "discovered_sources": 12,
    "imported_sources": 10,
    "partial_sources": 1,
    "failed_sources": 2,
    "dead_lettered_sources": 1,
    "persisted_rows": 12000,
    "quarantined_rows": 1,
    "failures": []
  }
}
```

说明：自动补偿只处理已到 `next_retry_at` 且未忽略、未进入 dead-letter 的来源。单项失败不会
阻断队列后续项；失败按 5 分钟起步指数退避，最多 8 次后进入 dead-letter。缺失发布时间或
开播时间的业务行不会用当前时间兜底，而是进入隔离区并把来源标记为 `partial`。

## 项目汇报

### `GET /api/v1/queries/projects`

一次性查询全部当前启用项目，仅返回前端选择项目所需的三个字段，不需要管理员令牌。

```json
[
  {
    "activity_period_id": 1,
    "project": "ROK",
    "period": "2026年7月第十四期"
  }
]
```

### `POST /api/v1/projects/{activity_period_id}/report/send`

读取项目 CPM 配置和数据库当前累计数据，并向请求指定的飞书群聊发送模板卡片。

请求体：

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `mission` | string | 是 | 今日事项 Markdown |
| `hot_videos` | string/null | 否 | 平台热点 Markdown；空白或不传时使用无热点模板 |
| `chat_id` | string | 是 | 接收消息的飞书群聊 ID |

模板选择：

- `hot_videos` 为空：`$PROJECT_REPORT_TEMPLATE_ID`
- `hot_videos` 非空：`$PROJECT_REPORT_WITH_HOT_VIDEOS_TEMPLATE_ID`

请求示例：

```bash
curl -X POST \
  "https://api.example.com/api/v1/projects/1/report/send" \
  -H "Content-Type: application/json" \
  -d '{
    "mission": "1. 跟进审核\n2. 汇总数据",
    "hot_videos": "- 平台热点内容",
    "chat_id": "oc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
  }'
```

卡片变量：

| 变量 | 数据来源 |
| --- | --- |
| `video_play` | 该期每个视频最新 `play_count` 的总和除以 10000，单位为万，保留两位小数 |
| `video_cpm` | CPM 表字段 `视频CPM`，保留两位小数 |
| `live_pv` | 该期全部直播 `live_exposure_pv` 总和除以 10000，单位为万，保留两位小数 |
| `live_cpm` | CPM 表字段 `直播CPM`，保留两位小数 |
| `date` | 接口触发时的北京时间日期，格式 `YYYY-MM-DD` |
| `mission` | 请求中的今日事项 Markdown |
| `hot_videos` | 请求中的平台热点 Markdown，允许空字符串 |
| `period` | 该项目的活动期数 |
| `project_name` | 该项目配置中的 `project` |

项目的 `cpm_table_id` 保存在 `xingtu_activity_period`。该表与本期业务表共用 `bitable_url` 中解析出的 app token，表内必须存在非空的 `直播CPM` 和 `视频CPM` 字段。

数据库中的播放量和直播 PV 始终保留原始整数；仅在项目汇报卡片和接口响应中按万换算并格式化。以上四个数值变量均以字符串返回，确保固定保留两位小数。

## 管理接口

管理接口需要请求头 `Authorization: Bearer <MUTATION_API_TOKEN>`；为兼容旧部署也可只配置
`ADMIN_API_TOKEN`。服务端两者都未配置时默认拒绝访问。浏览器可打开 `/admin-console` 使用
轻量内部控制台，token 只保存在页面内存。

### 飞书机器人群聊与成员

```text
GET /api/v1/admin/feishu/chats
GET /api/v1/admin/feishu/chats/{chat_id}/members
```

两个接口使用服务端配置的飞书应用身份和 tenant access token；调用方只提供本系统的管理员
Bearer Token，不传飞书 Token。响应会保留飞书官方的 HTTP 状态码以及完整 `code/data/msg`
JSON 信封，不转换成项目自己的列表结构。每次请求只代理一页，下一页继续传入响应中的
`page_token`，避免自动聚合改变官方分页语义。

查询机器人所在群聊：

```bash
curl "https://api.example.com/api/v1/admin/feishu/chats?user_id_type=union_id&sort_type=ByActiveTimeDesc&page_size=100" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"
```

| 参数 | 可选值/范围 | 说明 |
| --- | --- | --- |
| `user_id_type` | `open_id`、`union_id`、`user_id` | 返回群主 ID 的类型；不传时使用飞书默认值 |
| `sort_type` | `ByCreateTimeAsc`、`ByActiveTimeDesc` | 创建时间升序或活跃时间降序 |
| `page_size` | 1..100 | 飞书默认 20 |
| `page_token` | string | 上一页返回的分页标记 |

查询指定群聊成员：

```bash
curl "https://api.example.com/api/v1/admin/feishu/chats/oc_xxxxxxxxxxxxxxxx/members?member_id_type=union_id&page_size=100" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"
```

成员接口的 `member_id_type` 支持 `open_id`、`union_id`、`user_id`，还支持 `page_size` 和
`page_token`。机器人必须已在目标群内；飞书不会在该接口中返回机器人成员。使用 `user_id`
时还需要为应用开通相应的用户 ID 字段权限。部署前请在飞书开放平台开启机器人能力以及群信息/
群成员读取权限。官方参考：[获取用户或机器人所在的群列表](https://open.feishu.cn/document/server-docs/group/chat/list)、
[获取群成员列表](https://open.feishu.cn/document/server-docs/group/chat-member/get)。

### 根据多维表链接枚举数据表

```text
GET /api/v1/admin/feishu/bitable/tables?url=<飞书链接>&page_token=<可选>
```

接口接受 `/wiki/{app_token}` 或 `/base/{app_token}` 的飞书/Lark HTTPS 链接，也兼容直接复制的
Markdown 链接文本。服务端只提取路径中的 `app_token`，调用飞书
`GET /open-apis/bitable/v1/apps/{app_token}/tables`，并把 `page_size` 固定为 `99`。返回值保留
飞书官方 HTTP 状态及完整 `code/data/msg` 信封；如 `has_more=true`，将响应的 `page_token`
继续传给本接口读取下一页。

```bash
curl --get "https://api.example.com/api/v1/admin/feishu/bitable/tables" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN" \
  --data-urlencode "url=https://example.feishu.cn/wiki/ExampleBitableToken?table=tblExampleDataTable&view=vew3grghtl"
```

### 主项目配置

主项目把通知配置、星图账号、审核员和活动期次组织在同一个稳定的 `project_id` 下。
这些配置各自只有一个数据库归属，不再复制到账号或期次表：

```text
GET   /api/v1/admin/projects
POST  /api/v1/admin/projects
GET   /api/v1/admin/projects/{project_id}
PATCH /api/v1/admin/projects/{project_id}

GET   /api/v1/admin/projects/{project_id}/notification
PATCH /api/v1/admin/projects/{project_id}/notification

GET   /api/v1/admin/projects/{project_id}/accounts
POST  /api/v1/admin/projects/{project_id}/accounts
PATCH /api/v1/admin/projects/{project_id}/accounts/{xingtu_account_id}

GET   /api/v1/admin/projects/{project_id}/auditors
POST  /api/v1/admin/projects/{project_id}/auditors
PATCH /api/v1/admin/projects/{project_id}/auditors/{project_auditor_id}

GET   /api/v1/admin/projects/{project_id}/periods
POST  /api/v1/admin/projects/{project_id}/periods
GET   /api/v1/admin/projects/{project_id}/periods/{activity_period_id}
PUT   /api/v1/admin/projects/{project_id}/periods/{activity_period_id}
PATCH /api/v1/admin/projects/{project_id}/periods/{activity_period_id}/status
```

`GET /projects/{project_id}` 会同时返回 `notification`、`accounts`、`auditors` 和 `periods`。
创建项目时通知配置必填：

```json
{
  "project_key": "ROK",
  "display_name": "[ROK]生态",
  "is_active": true,
  "notification": {
    "notification_receive_id_type": "chat_id",
    "notification_receive_id": "oc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "audit_notice_card_template_id": "replace-with-audit-card-template-id",
    "audit_result_field": "审核结果",
    "login_notice_card_template_id": "replace-with-login-notice-card-template-id"
  }
}
```

账号和审核员分别通过其嵌套接口新增。一个项目最多一个默认账号；第一个账号会自动成为默认账号，
停用仍被启用期次引用的账号会返回 `409`。期次请求只包含期次自身字段、`xingtu_account_id`
和 `contents`；省略账号时使用项目默认账号。未知字段以及已经终结的旧通知字段会返回 `400`。
所有停用操作均保留历史数据。

### 卡片消息历史与撤回

错误日志、审核和日报卡片发送成功后，会保存消息摘要、发送时间、接收群信息和飞书 `message_id`。历史表不保存完整卡片内容或星图登录凭据。

```text
GET  /api/v1/admin/card-messages
POST /api/v1/admin/card-messages/{message_id}/recall
```

历史查询参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `category` | string | 可选：`error_log`、`audit`、`daily_report` |
| `date_from` | date | 可选，北京时间发送日期起点，闭区间 |
| `date_to` | date | 可选，北京时间发送日期终点，闭区间 |
| `limit` | integer | 默认 50，范围 1..500 |
| `offset` | integer | 默认 0 |

查询示例：

```bash
curl "https://api.example.com/api/v1/admin/card-messages?category=error_log&date_from=2026-08-01&date_to=2026-08-06&limit=100" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"
```

响应按 `sent_at` 从新到旧返回：

```json
[
  {
    "card_message_history_id": 12,
    "message_id": "om_xxxxxxxxxxxxxxxx",
    "category": "error_log",
    "summary": "ROK：星图数据拉取失败",
    "receive_id_type": "chat_id",
    "receive_id": "oc_xxxxxxxxxxxxxxxx",
    "project_name": "ROK",
    "activity_period_id": null,
    "sent_at": "2026-08-06T01:20:30Z",
    "last_recall_attempt_at": null,
    "recalled_at": null,
    "recall_error": null
  }
]
```

撤回示例：

```bash
curl -X POST \
  "https://api.example.com/api/v1/admin/card-messages/om_xxxxxxxxxxxxxxxx/recall" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"
```

撤回成功后接口返回更新后的历史记录，并写入 `recalled_at`。对已经撤回的记录再次调用会直接返回成功，不重复请求飞书。飞书拒绝撤回时接口返回错误，同时保存 `last_recall_attempt_at` 和 `recall_error`，便于排查消息权限或飞书撤回时限。

### 运行历史、失败补偿和隔离区

```text
GET  /api/v1/admin/status
GET  /api/v1/admin/periods/statuses
GET  /api/v1/admin/workflow-runs
GET  /api/v1/admin/workflow-runs/{workflow_run_id}/steps
GET  /api/v1/admin/failed-sources
POST /api/v1/admin/failed-sources/{feishu_source_id}/retry
POST /api/v1/admin/failed-sources/{feishu_source_id}/ignore
GET  /api/v1/admin/quarantine
```

重试接口只处理指定来源并受 PostgreSQL 全局工作流锁保护；忽略后自动队列不再处理该来源。
隔离行在同一业务键后续成功入库时自动标记 `resolved_at`。

活动状态更新示例：

```bash
curl -X PATCH "https://api.example.com/api/v1/admin/projects/1/periods/1/status" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "is_active": true,
    "need_trace": true,
    "morning_review_enabled": true,
    "periodic_sync_enabled": true
  }'
```

期次详情返回期次固有配置和其下直播/视频内容配置。旧 `/admin/activities*` 和全局
`/admin/auditors*` 已移除；这是 1.0.0 的不兼容收口，不提供别名路由。

## 查询接口

新管理前端应优先使用统一分页接口：

```text
GET /api/v1/queries/v2/videos
GET /api/v1/queries/v2/videos/label-summary
GET /api/v1/queries/v2/live-sessions
GET /api/v1/queries/v2/feishu-sources
```

它们统一返回 `{ "ok": true, "data": [], "meta": { "total", "has_more", "next_cursor" } }`，
分页接口统一支持 `activity_period_id`、`content_config_id`、`status`、`date_from`、`date_to`、
`search`、`limit`、`offset` 和优先级更高的 `cursor`。视频接口额外支持 `label`：该参数对
去除首尾空白后的完整标签做忽略大小写精确匹配；`search` 会同时模糊搜索视频 ID、标题、
作者名称和标签。旧裸数组接口继续保留兼容。

`date_from`/`date_to` 都按 `Asia/Shanghai` 业务日期解释。视频列表筛选发布时间，直播列表筛选
开播时间；开始日从 `00:00:00` 起，结束日包含到 `23:59:59` 以及其后的小数秒。实现使用
“结束日下一天 00:00:00 之前”的半开区间，因此不会漏掉带微秒的记录，也不会包含下一日零点。
若 `date_from > date_to`，接口返回 `400 bad_request`。

### `GET /api/v1/queries/projects`

精简项目列表见“项目汇报”章节。该接口返回当前启用期次的 `activity_period_id`、`project_id`、
`project_key`、`project_display_name` 和 `period`，无分页。

### `GET /api/v1/queries/periods`

查询启用中的活动期次配置。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/periods"
```

响应示例：

```json
[
  {
    "activity_period_id": 1,
    "project_id": 1,
    "project_key": "ROK",
    "project_display_name": "[ROK]生态",
    "period": "2026年7月第十四期",
    "period_code": "rok-2026-07-p14",
    "xingtu_account_id": "demo-xingtu-account",
    "task_month": "2026-07-01",
    "bitable_url": "https://example.feishu.cn/wiki/xxx",
    "need_trace": true,
    "morning_review_enabled": true,
    "periodic_sync_enabled": true,
    "periodic_sync_interval_hours": 2,
    "tracking_start_date": "2026-07-01",
    "tracking_end_date": "2026-08-15"
  }
]
```

`tracking_end_date` 表示业务追踪的最后日期，并额外包含北京时间 T+1 的 `03:00-03:59`
最终处理窗口。例如配置为 `2026-08-03`，最后一次更新发生在
`2026-08-04 03:00` 调度；当天 `09:00`、`15:00`、`21:00` 不再拉取星图，也不会再读取
旧 Sheet、更新业务多维表或发送自动审核通知。

### `GET /api/v1/queries/contents`

查询活动直播/视频内容配置。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/contents"
```

响应示例：

```json
[
  {
    "content_config_id": 1,
    "activity_period_id": 1,
    "period": "2026年7月第十四期",
    "content_type": "video",
    "xingtu_task_id": "demo-video-task-id",
    "xingtu_task_name": "短视频-万国觉醒2026年7月执政官创作营",
    "source_spreadsheet_url": "https://example.larksuite.com/sheets/ExampleSpreadsheetToken",
    "manual_table_id": "tblExampleVideoManual",
    "main_table_id": "tblExampleVideoMain",
    "audit_table_id": "tblExampleVideoAudit",
    "sync_enabled": true,
    "trace_enabled": true
  }
]
```

### `GET /api/v1/queries/feishu-sources`

查询星图导出的飞书 Sheet 来源。

查询参数：支持 `limit`、`offset`。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/feishu-sources?limit=50&offset=0"
```

响应示例：

```json
[
  {
    "feishu_source_id": 101,
    "content_config_id": 1,
    "content_type": "video",
    "feishu_sheet_url": "https://example.larksuite.com/sheets/ExampleSpreadsheetToken",
    "trigger_type": "morning",
    "stat_date": "2026-07-05",
    "pulled_at": "2026-07-05T10:00:12+08:00",
    "is_daily_final": false,
    "import_status": "imported",
    "imported_row_count": 2660
  }
]
```

`import_status` 常见值：

| 值 | 说明 |
| --- | --- |
| `pending` | 已拿到链接，待导入 |
| `imported` | 已导入 |
| `failed` | 导入失败，可通过补偿导入重试 |
| `partial` | 有效行已导入，缺失业务时间等异常行已进入隔离区 |

### `GET /api/v1/queries/pending-summary`

查询飞书来源待导入/失败数量。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/pending-summary"
```

响应示例：

```json
{
  "pending_sources": 2,
  "failed_sources": 1
}
```

### `GET /api/v1/queries/videos`

查询视频/图文基础内容数据。

查询参数：支持 `limit`、`offset`。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/videos?limit=50&offset=0"
```

响应示例：

```json
[
  {
    "content_config_id": 1,
    "video_id": "demo-content-id",
    "publish_time": "2026-07-05T09:30:00",
    "author_name": "作者名称",
    "author_uid": "123456",
    "title": "视频标题",
    "audit_result": null,
    "label": null,
    "last_seen_at": "2026-07-05T10:02:30+08:00"
  }
]
```

### `GET /api/v1/queries/video-metrics`

查询视频每日追踪指标。

查询参数：支持 `limit`、`offset`。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/video-metrics?limit=50&offset=0"
```

响应示例：

```json
[
  {
    "content_config_id": 1,
    "stat_date": "2026-07-05",
    "video_id": "demo-content-id",
    "play_count": 1200,
    "valid_play_count": 900,
    "like_count": 88,
    "comment_count": 12,
    "share_count": 6,
    "is_daily_final": false,
    "imported_at": "2026-07-05T10:03:12+08:00"
  }
]
```

说明：当前只有视频写入每日追踪指标；直播只保存最新场次数据。星图中不存在的手动登记视频同步时也会写入播放量快照：`播放量`有值时保存实际值，缺失时保存 `0`，其他未提供的指标保持为 `null`。同一稿件同时存在于星图和手动登记表时使用星图数据。

### `GET /api/v1/queries/video-trace-metrics`

查询视频每次星图追踪导入的快照指标。这个接口读取历史导入表，适合查看 03:00、09:00、15:00、21:00、23:59 固定拉取和手动触发留下的快照数据。

查询参数：支持 `content_config_id`、`video_id`、`author_uid`、`author_name`、`label`、`date`、`date_from`、`date_to`、`limit`、`offset`。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/video-trace-metrics?content_config_id=1&video_id=demo-content-id&date=2026-07-05&limit=100"
```

响应示例：

```json
[
  {
    "feishu_source_id": 12,
    "content_config_id": 1,
    "stat_date": "2026-07-05",
    "video_id": "demo-content-id",
    "trigger_type": "periodic",
    "pulled_at": "2026-07-05T12:01:00+08:00",
    "is_daily_final": false,
    "play_count": 1200,
    "valid_play_count": 900,
    "like_count": 88,
    "valid_like_count": 70,
    "comment_count": 12,
    "share_count": 6,
    "component_click_count": 3,
    "android_activate_count": 1,
    "ios_activate_count": 0,
    "reservation_success_count": 0,
    "reservation_install_complete_count": 0,
    "follower_increase_count": 4,
    "like_rate": 0.073333,
    "comment_rate": 0.01,
    "imported_at": "2026-07-05T12:03:12+08:00"
  }
]
```

### `GET /api/v1/queries/live-sessions`

查询直播场次最新数据。

查询参数：支持 `limit`、`offset`。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/live-sessions?limit=50&offset=0"
```

响应示例：

```json
[
  {
    "content_config_id": 2,
    "live_room_id": "demo-content-id",
    "start_time": "2026-07-05T20:00:00",
    "anchor_name": "主播名称",
    "anchor_uid": "123456",
    "title": "直播标题",
    "cumulative_viewer_count": 5600,
    "live_exposure_pv": 8800,
    "acu": 82.5,
    "audit_result": null,
    "last_seen_at": "2026-07-05T23:59:59+08:00"
  }
]
```

说明：`live_exposure_pv` 来自星图字段 `直播曝光pv`，业务展示名为“场观PV”；`cumulative_viewer_count` 继续保留，用于兼容旧数据和旧前端。

## 分析查询接口

分析查询接口都支持 5 分钟服务端缓存；任一写工作流结束时无论最终成功或失败都会失效缓存，
数据库版本号会让其他实例同时绕过旧缓存。

视频通用筛选参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `content_config_id` | integer | 限定某一期活动的视频配置 |
| `video_id` | string | 精确筛选视频 ID，仅视频详情/汇总接口支持 |
| `author_uid` | string | 精确筛选作者 uid |
| `author_name` | string | 模糊筛选作者名称 |
| `label` | string | 忽略大小写精确筛选审核标签 |
| `date` | date | 指定单日，优先于 `date_from`/`date_to` |
| `date_from` | date | 日期区间开始，闭区间 |
| `date_to` | date | 日期区间结束，闭区间 |
| `limit` | integer | 返回条数，详情接口默认 50，增长榜默认 20 |
| `offset` | integer | 分页偏移，仅详情接口支持 |

视频指标类接口的 `date`/`date_from`/`date_to` 按北京时间业务日筛选 `stat_date`，范围两端均
包含；发布时间列表则按上文的 `00:00:00` 至 `23:59:59` 语义筛选。所有接口都会拒绝反向日期
范围。

### `GET /api/v1/queries/v2/videos/label-summary`

按审核标签生成视频周报数据。`date_from` 和 `date_to` 必填，最长允许 366 天；还可使用
`activity_period_id`、`content_config_id`、`status` 和 `label` 缩小范围。空或全空白标签会单独
归到 `label=null`、`label_name="未标注"`。

- `published_video_count`：发布时间落在北京时间范围内的稿件数。
- `active_video_count`：范围内至少出现过一天指标快照的稿件数。
- `author_count`：该标签下上述发布或活跃稿件涉及的去重作者数。
- `*_growth`：对每日累计快照与前一日快照做非负差值后，在范围内求和。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/v2/videos/label-summary?activity_period_id=1&date_from=2026-08-03&date_to=2026-08-09"
```

响应示例：

```json
{
  "ok": true,
  "range": {
    "timezone": "Asia/Shanghai",
    "date_from": "2026-08-03",
    "date_to": "2026-08-09",
    "start_at": "2026-08-03T00:00:00",
    "end_at": "2026-08-09T23:59:59"
  },
  "data": [
    {
      "label": "攻略",
      "label_name": "攻略",
      "published_video_count": 18,
      "active_video_count": 42,
      "author_count": 31,
      "play_growth": 860000,
      "valid_play_growth": 610000,
      "like_growth": 23000,
      "comment_growth": 1700,
      "share_growth": 920
    },
    {
      "label": null,
      "label_name": "未标注",
      "published_video_count": 2,
      "active_video_count": 3,
      "author_count": 3,
      "play_growth": 12000,
      "valid_play_growth": 8000,
      "like_growth": 320,
      "comment_growth": 28,
      "share_growth": 16
    }
  ],
  "total": {
    "published_video_count": 20,
    "active_video_count": 45,
    "play_growth": 872000,
    "valid_play_growth": 618000,
    "like_growth": 23320,
    "comment_growth": 1728,
    "share_growth": 936
  }
}
```

`total` 中不提供作者数，因为同一作者可能出现在多个标签下，直接相加会重复计数。若只需要
某个标签，可传 `label=攻略`；标签筛选同样会命中标签检索索引。

### `GET /api/v1/queries/videos/with-metrics`

查询视频基础信息 + 每日线性指标。`metrics` 按 `stat_date` 升序返回，并包含按前一日快照计算的每日增量。手动登记视频完成同步后也会包含播放量指标；播放量字段缺失时为 `0`，有值时返回实际数值。尚未重新同步的历史手动视频仍会返回基础信息，此时 `metrics` 可能为空数组。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/videos/with-metrics?content_config_id=1&date_from=2026-07-01&date_to=2026-07-07&limit=20"
```

响应示例：

```json
[
  {
    "content_config_id": 1,
    "video_id": "demo-content-id",
    "publish_time": "2026-07-05T09:30:00",
    "author_name": "作者名称",
    "author_uid": "123456",
    "title": "视频标题",
    "audit_result": null,
    "label": null,
    "latest_play_count": 1800,
    "latest_like_count": 120,
    "latest_comment_count": 16,
    "metrics": [
      {
        "stat_date": "2026-07-05",
        "play_count": 1200,
        "valid_play_count": 900,
        "like_count": 88,
        "comment_count": 12,
        "share_count": 6,
        "play_increment": 300,
        "like_increment": 20,
        "comment_increment": 3,
        "is_daily_final": false,
        "imported_at": "2026-07-05T10:03:12+08:00"
      }
    ]
  }
]
```

### `GET /api/v1/queries/videos/summary`

查询视频播放量、点赞量、评论量、稿件数量、作者数量汇总。播放/点赞/评论按每日增量口径汇总，避免区间内多天快照重复计算。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/videos/summary?content_config_id=1&date_from=2026-07-01&date_to=2026-07-07"
```

响应示例：

```json
{
  "play_count": 350000,
  "like_count": 12000,
  "comment_count": 900,
  "content_count": 2660,
  "author_count": 1800
}
```

直播通用筛选参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `content_config_id` | integer | 限定某一期活动的直播配置 |
| `anchor_uid` | string | 精确筛选主播 uid |
| `anchor_name` | string | 模糊筛选主播名称 |
| `date` | date | 指定开播日期，优先于 `date_from`/`date_to` |
| `date_from` | date | 开播日期区间开始，闭区间 |
| `date_to` | date | 开播日期区间结束，闭区间 |

### `GET /api/v1/queries/lives/summary`

查询直播场观 PV、ACU 平均值、直播场次数量、主播数量汇总。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/lives/summary?content_config_id=2&date=2026-07-05"
```

响应示例：

```json
{
  "live_exposure_pv": 560000,
  "avg_acu": 82.5,
  "live_session_count": 54,
  "anchor_count": 48
}
```

### `GET /api/v1/queries/videos/top-growth`

查询指定范围内播放量增长最快的前 N 条内容。增长量按每日播放增量求和。

请求示例：

```bash
curl "https://api.example.com/api/v1/queries/videos/top-growth?content_config_id=1&date_from=2026-07-01&date_to=2026-07-07&limit=10"
```

响应示例：

```json
[
  {
    "content_config_id": 1,
    "video_id": "demo-content-id",
    "publish_time": "2026-07-05T09:30:00",
    "author_name": "作者名称",
    "author_uid": "123456",
    "title": "视频标题",
    "audit_result": null,
    "label": null,
    "play_growth": 120000,
    "avg_daily_play_growth": 17142.85,
    "metric_days": 7,
    "latest_play_count": 350000,
    "latest_stat_date": "2026-07-07"
  }
]
```

## audit_extra 扩展查询

### `POST /api/v1/queries/v2/audit-extra/search`

按主项目、活动期次和一个或多个 `audit_extra` 条件查询视频、直播最新详情及汇总。多个条件使用 AND，字符串、数字和布尔值按业务 JSON 类型精确匹配。

飞书返回的 `{ "type": 1, "value": [{ "text": "..." }] }` 传输包装会在入库前转换为普通字符串；数字和布尔值也会去掉 `type/value` 外壳并保留原类型。因此客户端只传业务值，不要传飞书富文本结构。

顶层字段名 `key` 属于机密字段：可以作为筛选条件，但不会出现在过滤条件回显、视频详情、直播详情或查询缓存中。

```bash
curl -X POST "https://api.example.com/api/v1/queries/v2/audit-extra/search" \
  -H "Content-Type: application/json" \
  -d '{
    "project_id": 1,
    "activity_period_id": 2,
    "conditions": {
      "rok_key": "ROK",
      "key": "<保密值>"
    },
    "audit_result": "审核通过",
    "limit": 100,
    "offset": 0
  }'
```

汇总中的 `total_play_count` 是每个命中视频最新指标快照的播放量之和；`total_live_exposure_pv` 只统计直播的 `live_exposure_pv`。

### `POST /api/v1/queries/v2/audit-extra/live-pv/weighted-acu-below`

先在指定主项目和活动期次中应用可选的 `audit_extra` 条件与审核结果，再按非空 `anchor_uid` 聚合直播数据。每个 UID 的加权平均 ACU 计算方式为：

```text
Σ(acu × live_duration_seconds) / Σ(live_duration_seconds)
```

只有同时具有 ACU 且 `live_duration_seconds > 0` 的场次参与加权平均计算。完全没有有效 ACU/时长组合的 UID 不参与门槛判断，避免把未知数据误判为低 ACU。筛出加权平均 ACU 严格小于 `weighted_average_acu_lt` 的 UID 后，接口汇总这些 UID 在相同 `audit_extra`、审核结果和期次范围内全部场次的 `live_exposure_pv`；选中 UID 缺少 ACU 的其他场次仍计入最终 PV。

`conditions` 可以省略或传 `{}`，表示不限制 `audit_extra`。顶层机密条件 `key` 仍可用于筛选，但不会在响应中回显，也不会进入查询缓存键。

```bash
curl -X POST "https://api.example.com/api/v1/queries/v2/audit-extra/live-pv/weighted-acu-below" \
  -H "Content-Type: application/json" \
  -d '{
    "project_id": 1,
    "activity_period_id": 2,
    "conditions": {
      "rok_key": "ROK"
    },
    "audit_result": "审核通过",
    "weighted_average_acu_lt": 10
  }'
```

响应示例：

```json
{
  "ok": true,
  "scope": {
    "project_id": 1,
    "project_key": "rok",
    "project_display_name": "ROK 生态",
    "activity_period_id": 2,
    "period": "2026年8月第十五期",
    "period_code": "rok-2026-08-p15"
  },
  "filters": {
    "conditions": {
      "rok_key": "ROK"
    },
    "audit_result": "审核通过",
    "weighted_average_acu_lt": 10.0
  },
  "summary": {
    "candidate_user_count": 38,
    "evaluated_user_count": 36,
    "selected_user_count": 9,
    "selected_live_session_count": 21,
    "total_live_exposure_pv": 582100
  }
}
```

字段说明：

- `candidate_user_count`：筛选范围内具有非空 `anchor_uid` 的 UID 数量。
- `evaluated_user_count`：至少有一场可参与加权计算的 UID 数量。
- `selected_user_count`：加权平均 ACU 严格低于门槛的 UID 数量。
- `selected_live_session_count`：选中 UID 在筛选范围内的全部直播场次数。
- `total_live_exposure_pv`：这些直播场次的业务场观 PV 总和，只使用 `live_exposure_pv`。

### `POST /api/v1/queries/v2/audit-extra/video-play/author-total-below`

先在指定主项目和活动期次中应用可选的 `audit_extra` 条件与审核结果，再按非空 `author_uid` 聚合视频。每条视频只取 `stat_date` 最新、同日 `imported_at` 最新的一条 `video_daily_metric`，然后将最新 `play_count` 按 UID 相加。

筛出 UID 总播放量严格小于 `author_total_play_count_lt` 的用户后，接口返回这些 UID 的视频数量和总播放量。没有指标或最新 `play_count` 为空的视频按 `0` 参与聚合，因此在正数门槛下，对应 UID 也可能被选中。空 `author_uid` 不参与统计。

```bash
curl -X POST "https://api.example.com/api/v1/queries/v2/audit-extra/video-play/author-total-below" \
  -H "Content-Type: application/json" \
  -d '{
    "project_id": 1,
    "activity_period_id": 2,
    "conditions": {
      "rok_key": "ROK"
    },
    "audit_result": "审核通过",
    "author_total_play_count_lt": 100000
  }'
```

响应示例：

```json
{
  "ok": true,
  "scope": {
    "project_id": 1,
    "project_key": "rok",
    "project_display_name": "ROK 生态",
    "activity_period_id": 2,
    "period": "2026年8月第十五期",
    "period_code": "rok-2026-08-p15"
  },
  "filters": {
    "conditions": {
      "rok_key": "ROK"
    },
    "audit_result": "审核通过",
    "author_total_play_count_lt": 100000
  },
  "summary": {
    "candidate_user_count": 143,
    "selected_user_count": 51,
    "selected_video_count": 126,
    "selected_video_with_metric_count": 121,
    "total_play_count": 1836500
  }
}
```

字段说明：

- `candidate_user_count`：筛选范围内具有非空 `author_uid` 的 UID 数量。
- `selected_user_count`：UID 最新视频总播放严格低于门槛的用户数量。
- `selected_video_count`：选中 UID 在筛选范围内的视频数量。
- `selected_video_with_metric_count`：选中视频中至少具有一个指标快照的视频数量。
- `total_play_count`：选中 UID 的视频最新播放量总和。

## 缓存说明

查询接口使用 5 分钟服务端内存缓存。缓存值是 zstd level 1 压缩后的 JSON，并按压缩后的实际
字节数执行 128 MiB 容量淘汰；命中时在服务端透明解压，客户端响应结构不变。损坏条目会被自动
丢弃并重新查询。缓存通过 PostgreSQL revision 实现跨实例失效。以下写操作
结束后无论最终成功或失败都会尝试失效缓存，避免部分提交后继续展示旧数据：

- `POST /api/v1/xingtu/sessions`
- `POST /api/v1/workflows/{kind}/run`
- `POST /api/v1/workflows/manual-sync/run`
- `POST /api/v1/workflows/audit-results/sync`
- `POST /api/v1/workflows/pending/import`

调度器每轮执行结束后也会失效共享缓存。

## 常用调试命令

```bash
# 健康检查
curl "https://api.example.com/health"

# 上传星图登录态
curl -X POST "https://api.example.com/api/v1/xingtu/sessions" \
  -H "Authorization: Bearer $XINGTU_SESSION_UPLOAD_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"xingtu_account_id":"demo-xingtu-account","cookie":"...","csrf_token":"..."}'

# 执行每日早工作流
curl -X POST "https://api.example.com/api/v1/workflows/morning/run" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"

# 手动将审核表结果同步到数据库
curl -X POST "https://api.example.com/api/v1/workflows/audit-results/sync" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"

# 只同步直播、视频手动登记数据
curl -X POST "https://api.example.com/api/v1/workflows/manual-sync/run" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"

# 补偿导入 pending/failed 来源
curl -X POST "https://api.example.com/api/v1/workflows/pending/import" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"limit":200}'
```
