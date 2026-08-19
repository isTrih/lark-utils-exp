# 运维、备份与故障恢复手册

## 服务探针与业务健康

- `GET /live`：只证明进程仍可响应，供 Docker/Kubernetes liveness 使用。
- `GET /ready`：检查 PostgreSQL 和关键内部表，供 readiness 使用。
- `GET /health`：返回数据库连通性和完整构建版本。
- `GET /api/v1/admin/status`：需 Bearer token，展示运行中任务、failed/dead-letter、隔离数据、失效账号和最近成败时间。
- `/admin-console`：轻量内部控制台，token 只保存在当前页面内存，不写入浏览器存储。

建议告警阈值：`running_workflows > 0` 持续 6 小时、`dead_letter_sources > 0`、
`unresolved_quarantine_rows > 0` 持续 24 小时、`invalid_accounts > 0`，或最近成功时间超过预期调度间隔。
容器 liveness 不依赖飞书/星图，避免外部短暂故障导致重启风暴。

## PostgreSQL 备份

主机需安装与数据库主版本兼容的 `pg_dump`/`pg_restore`，备份目录必须加密并限制访问：

```bash
DATABASE_URL='postgres://...' \
  bun scripts/postgres-backup.ts backup /srv/backups/lark-utils-$(date +%F-%H%M).dump
```

至少每日备份，保留 7 个日备、4 个周备；生产环境再启用 PostgreSQL WAL/PITR。每月至少在隔离数据库做一次恢复演练：

```bash
createdb lark_utils_restore_drill
DATABASE_URL='postgres://.../lark_utils_restore_drill' \
  bun scripts/postgres-backup.ts restore /srv/backups/lark-utils-YYYY-MM-DD-HHMM.dump --confirm-restore
cargo run --bin server
curl --fail http://127.0.0.1:8080/ready
```

恢复目标必须是空的演练库或已经明确批准覆盖的故障库。应用启动会自动执行向前 migration；
不要修改已经执行过的 migration。回退镜像前先核对旧二进制是否能读取新结构，必要时优先恢复数据库备份，
而不是直接执行 down migration。包含密文登录态时，回退到明文字段版本会被 down migration 主动拒绝。

## 密钥备份与轮换

`XINGTU_SESSION_ENCRYPTION_KEY` 是登录态密文的解密根；遗失后数据库备份中的星图登录态无法恢复。
密钥必须保存到部署平台 Secret/KMS，并与数据库备份分开备份。不能直接替换密钥后重启；正确流程是：

1. 保持旧密钥运行，重新通过 `POST /api/v1/xingtu/sessions` 上传各账号登录态。
2. 在维护窗口部署新密钥并重新上传所有登录态，使数据库只保留新密钥产生的密文。
3. 验证账号检查成功、完成一次数据库备份后，才销毁旧密钥。

API 响应保护密钥的客户端分发与解密见 [数据保护说明](./data-protection.md)。

## 发布、回滚与并发

正式镜像只能在完成 `fmt/check/test/diff-check`、Git commit 后通过
`bun scripts/docker-release.ts` 发布。推荐先启动新实例，确认 `/ready` 后再停止旧实例。
PostgreSQL advisory lock 会阻止多副本重复执行写工作流；被阻止的调用会写入 `blocked` 台账。

回滚步骤：保留当前容器和不可变镜像标签，切回上一个不可变标签，确认 `/ready` 与完整构建版本，
再观察管理台的最近任务、failed 队列和隔离区。若新 migration 与旧程序不兼容，按已演练的数据库恢复方案处理。

## 日志与容量

Compose 示例同时限制 Docker stdout 日志大小，并把 `/app/logs` 挂载到持久卷。应用 JSONL 日志可定时清理：

```bash
LOG_DIR=/srv/lark-utils-exp/logs LOG_RETENTION_DAYS=30 \
  bun scripts/prune-logs.ts --dry-run
LOG_DIR=/srv/lark-utils-exp/logs LOG_RETENTION_DAYS=30 \
  bun scripts/prune-logs.ts
```

对日志卷使用率 70%/85%、数据库磁盘 70%/85%、连接池耗尽和备份失败设置告警。
清理前先确认日志已进入集中日志系统或满足内部保留策略。
