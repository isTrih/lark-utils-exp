# Docker 部署说明

## 镜像

当前服务镜像默认发布到 Docker Hub：

```bash
trihlp/lark-utils-exp
```

基础版本以 `Cargo.toml` 的 `package.version` 为唯一来源，并遵循 SemVer：

- 不兼容的接口或配置变更：升级主版本。
- 向后兼容的新功能：升级次版本。
- 向后兼容的问题修复：升级修订版本。

Docker 发布统一使用 Bun 脚本：

```bash
bun scripts/docker-release.ts
```

脚本默认构建 `linux/amd64` 并发布三个标签：`latest`、基础 SemVer（例如 `0.2.0`），
以及包含构建时间和 Git 提交的不可变标签。二进制完整版本也会自动加入 SemVer build
metadata，例如 `0.2.0+build.20260806T010203Z.git.fa2eb7ca7c9e`。发布脚本默认拒绝
存在未提交的已跟踪文件，`--dry-run --allow-dirty` 可用于本地检查最终命令但不会构建。

可通过 `DOCKER_IMAGE` 和 `DOCKER_PLATFORM` 覆盖默认镜像仓库与平台。

## 必填环境变量

服务启动会执行数据库 migration，并连接飞书、星图等外部服务。部署时至少需要：

```bash
DATABASE_URL=postgres://user:password@host:5432/database
LARK_APP_ID=cli_xxx
LARK_APP_SECRET=xxx
MUTATION_API_TOKEN=replace-with-a-long-random-secret
# 使用星图同步插件时必填；插件包共用，只授权上传登录态。
XINGTU_SESSION_UPLOAD_TOKEN=replace-with-a-different-long-random-secret
# 标准 Base64 的 32 字节随机密钥；用于星图登录态落库加密。
XINGTU_SESSION_ENCRYPTION_KEY=replace-with-32-byte-standard-base64-key
XINGTU_SESSION_ENCRYPTION_KEY_ID=primary
```

## 可选环境变量

```bash
SERVER_BIND_ADDR=0.0.0.0:8080
RUST_LOG=info
LOG_DIR=/app/logs
TZ=Asia/Shanghai
LARK_BASE_URL=https://open.feishu.cn
CORS_DOMAIN=["example.com","*.example.com"]
# 旧部署兼容项；未配置 MUTATION_API_TOKEN 时才回退使用。
# ADMIN_API_TOKEN=replace-with-a-long-random-secret

# 可选：为显式请求 X-Data-Protection 的 /api/v1 客户端保护 JSON 响应。
# API_DATA_ENCRYPTION_KEY 必须是标准 Base64 编码的 32 字节随机密钥。
API_DATA_ENCRYPTION_KEY=replace-with-32-byte-standard-base64-key
API_DATA_ENCRYPTION_KEY_ID=primary

# 飞书多维表格数据同步插件。
# 飞书要求配置页 URL 使用 HTTPS。
DATA_SYNC_PUBLIC_BASE_URL=https://api.example.com
DATA_SYNC_SECRET_KEY=replace-with-the-verification-token

# 项目汇报卡片模板；仅调用项目汇报发送接口时需要。
PROJECT_REPORT_TEMPLATE_ID=replace-with-project-report-template-id
PROJECT_REPORT_WITH_HOT_VIDEOS_TEMPLATE_ID=replace-with-hot-video-report-template-id

# 如配置页单独部署，可用完整地址覆盖上面的自动拼接结果。
# DATA_SYNC_CONFIG_UI_URL=https://static.example.com/data-sync/config

# 调试期可通过环境变量在启动时注入星图登录态。
# 企业后端接入后，推荐通过 POST /api/v1/xingtu/sessions 写入。
XINGTU_ACCOUNT_ID=demo-xingtu-account
XINGTU_COOKIE=passport_auth_status_ss=...
XINGTU_CSRF_TOKEN=...
XINGTU_SESSION_KEY=...
XINGTU_USER_AGENT=Mozilla/5.0 ...
```

`XINGTU_COOKIE` 和 `XINGTU_CSRF_TOKEN` 同时存在时，服务启动会自动把登录态注入到
`XINGTU_ACCOUNT_ID`；此时账号 ID 也必须显式配置，避免把调试登录态写入错误账号。

`CORS_DOMAIN` 推荐使用 JSON 数组。域名规则不包含协议、端口或路径；主域和子域通配规则
需要分别声明。未配置时服务仍可启动，但不会为跨域请求返回许可；配置为空、非法或使用全开放
`*` 时服务会拒绝启动。

`XINGTU_SESSION_ENCRYPTION_KEY` 必须为规范标准 Base64，解码后恰好 32 字节。它是必填项；
如果未单独配置，服务会回退使用 `API_DATA_ENCRYPTION_KEY`。登录态以 AES-256-GCM 信封密文
写入 PostgreSQL，账号 ID 参与完整性校验；历史明文会在启动恢复后自动重写为密文。

`XINGTU_SESSION_UPLOAD_TOKEN` 在使用内部浏览器插件时是必填项，所有插件包可以共用同一个值；
它只能上传登录态，不能触发工作流或访问管理接口。不要把它设置成
`XINGTU_SESSION_ENCRYPTION_KEY` 或 `MUTATION_API_TOKEN`：上传 Token 会进入插件包，服务端
数据库加密根密钥和管理员 Token 则不应进入前端。服务端仍接受 `MUTATION_API_TOKEN` 直接调用
上传接口，方便管理员调试。插件私有配置、按人员/账号生成 ZIP 和共享 Token 轮换方式见
[`browser-extensions/xingtu-session-uploader/README.md`](../browser-extensions/xingtu-session-uploader/README.md)。

仓库根目录提供 [`.env.example`](../.env.example)。部署时复制为不会提交到 Git 的 `.env`：

```bash
cp .env.example .env
```

生成三个互不相同的随机凭据：

```bash
bun -e 'console.log(Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64url"))'
bun -e 'console.log(Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64url"))'
bun -e 'console.log(Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64"))'
```

前两个分别填写 `MUTATION_API_TOKEN`、`XINGTU_SESSION_UPLOAD_TOKEN`；第三个标准 Base64 值
填写 `XINGTU_SESSION_ENCRYPTION_KEY`。

内部前端可使用管理员 Token 读取插件上传 Token：

```bash
curl "https://api.example.com/api/v1/admin/xingtu/session-upload-token" \
  -H "Authorization: Bearer $MUTATION_API_TOKEN"
```

响应包含 `xingtu_session_upload_token`、固定的 `upload_path` 和 `token_type`，并带有
`Cache-Control: no-store, private` 与 `Pragma: no-cache`。该接口不会返回
`XINGTU_SESSION_ENCRYPTION_KEY`。

`DATA_SYNC_PUBLIC_BASE_URL` 用于生成 `/meta.json` 中的配置页完整地址，生产环境应显式配置，
例如 `https://api.example.com`。该值只填写协议、域名和可选端口，不包含路径；配置页和连接器
接口固定使用服务根路径。连接器地址始终规范化为 HTTPS，即使后端收到 HTTP 请求也不会返回
HTTP 配置页地址；未配置公开地址时，服务会从标准 `Forwarded` 或 `X-Forwarded-Host`
反向代理请求头识别外部 Host。`DATA_SYNC_SECRET_KEY` 需要与飞书数据同步插件调试页或上架配置
中的 Verification token 完全一致。

`API_DATA_ENCRYPTION_KEY` 未配置时，不携带数据保护请求头的现有客户端仍按原格式工作；显式请求 `X-Data-Protection: aes-256-gcm+zstd` 的客户端会收到 503。如果配置了密钥，它必须是规范标准 Base64 且解码后恰好 32 字节，否则服务拒绝启动。`API_DATA_ENCRYPTION_KEY_ID` 默认为 `primary`。生成密钥、安全分发、轮换和 Bun 解密说明见 [API 数据保护与解密](./data-protection.md)。该机制仅用于 `/api/v1/**` JSON 响应，飞书 `/api/data-sync/*` 插件协议不使用应用层信封。

## 持久化日志

服务同时输出控制台日志和按天滚动的 JSONL 文件。`RUST_LOG` 同时控制两种输出的日志级别，`LOG_DIR` 控制文件目录：

```text
/app/logs/lark-utils-exp.jsonl.YYYY-MM-DD
```

本地直接运行时，未配置或配置空的 `LOG_DIR` 表示只输出控制台日志。Docker 镜像默认使用 `/app/logs`；为了在删除或替换容器后保留日志，生产部署必须把该目录挂载到宿主机或命名卷。

宿主机目录示例：

```bash
sudo install -d -o 10001 -g 10001 /srv/lark-utils-exp/logs
```

容器使用 UID `10001` 运行，因此绑定宿主机目录时需要保证该 UID 可写。日志目录迁移时停止旧容器，将 `/srv/lark-utils-exp/logs` 复制到新服务器，再把新目录挂载到相同的 `LOG_DIR` 即可；每行都是独立 JSON，方便后续导入日志平台。

JSONL 的 `timestamp` 使用 UTC RFC 3339，便于跨服务器汇总；业务调度日志中的计划时间仍按北京时间计算。文件按天滚动，应用当前不自动删除历史日志，生产环境应在宿主机配置归档或定期清理，避免长期占满磁盘。

## 运行示例

仓库提供带健康探针、只读根文件系统、持久日志卷及 CPU/内存/PID 限制的
[`compose.example.yml`](../compose.example.yml)：

```bash
cp compose.example.yml compose.yml
docker compose up -d
docker compose ps
```

```bash
docker run -d --name lark-utils-exp \
  --restart unless-stopped \
  -p 8080:8080 \
  -e SERVER_BIND_ADDR=0.0.0.0:8080 \
  -e RUST_LOG=info \
  -e LOG_DIR=/app/logs \
  -e TZ=Asia/Shanghai \
  -e CORS_DOMAIN='["example.com","*.example.com"]' \
  -e MUTATION_API_TOKEN='replace-with-a-long-random-secret' \
  -e XINGTU_SESSION_UPLOAD_TOKEN='replace-with-a-long-random-secret' \
  -e XINGTU_SESSION_ENCRYPTION_KEY='replace-with-32-byte-standard-base64-key' \
  -e XINGTU_SESSION_ENCRYPTION_KEY_ID='primary' \
  -e API_DATA_ENCRYPTION_KEY='replace-with-32-byte-standard-base64-key' \
  -e API_DATA_ENCRYPTION_KEY_ID='primary' \
  -e DATA_SYNC_PUBLIC_BASE_URL='https://api.example.com' \
  -e DATA_SYNC_SECRET_KEY='replace-with-the-verification-token' \
  -e DATABASE_URL='postgres://user:password@host:5432/database' \
  -e LARK_APP_ID='cli_xxx' \
  -e LARK_APP_SECRET='xxx' \
  -e XINGTU_ACCOUNT_ID='demo-xingtu-account' \
  -e XINGTU_COOKIE='...' \
  -e XINGTU_CSRF_TOKEN='...' \
  -e XINGTU_SESSION_KEY='...' \
  -e XINGTU_USER_AGENT='Mozilla/5.0 ...' \
  -v /srv/lark-utils-exp/logs:/app/logs \
  trihlp/lark-utils-exp:latest
```

健康检查：

```bash
curl http://127.0.0.1:8080/health
curl --fail http://127.0.0.1:8080/live
curl --fail http://127.0.0.1:8080/ready
```

服务启动日志会输出完整版本、基础版本、Git 提交和镜像构建时间；`/health` 也返回同一组
构建信息，便于确认服务器实际运行的镜像。

镜像内置 Docker `HEALTHCHECK`，使用 `/live`，不会因飞书或星图短暂不可用而重启容器。
数据库就绪与完整的备份、恢复、滚动发布、回滚、日志保留和容量告警说明见
[运维、备份与故障恢复手册](./operations-runbook.md)。

## 常用接口

完整输入/返回说明见：[HTTP API 说明](./api.md)。

```text
GET  /health
GET  /live
GET  /ready
GET  /admin-console
GET  /meta.json
GET  /data-sync/config
POST /api/data-sync/table-meta
POST /api/data-sync/records
POST /api/v1/xingtu/sessions
GET  /api/v1/xingtu/sessions/{account_id}/check
POST /api/v1/workflows/morning/run
POST /api/v1/workflows/periodic/run
POST /api/v1/workflows/night/run
POST /api/v1/workflows/audit-results/sync
GET  /api/v1/admin/projects
POST /api/v1/admin/projects
GET  /api/v1/admin/projects/{project_id}
PATCH /api/v1/admin/projects/{project_id}
GET  /api/v1/admin/projects/{project_id}/accounts
GET  /api/v1/admin/projects/{project_id}/auditors
GET  /api/v1/admin/projects/{project_id}/periods
GET  /api/v1/admin/periods/statuses
GET  /api/v1/admin/status
GET  /api/v1/admin/xingtu/session-upload-token
GET  /api/v1/admin/workflow-runs
GET  /api/v1/admin/failed-sources
GET  /api/v1/admin/quarantine
GET  /api/v1/queries/periods
GET  /api/v1/queries/pending-summary
```

## 从 0.x 升级到 1.0.0

1.0.0 会删除期次、账号和审核员表中的旧项目/通知冗余列，并删除
`xingtu_activity_auditor`。执行 migration 前必须先做数据库备份并安排维护窗口。
迁移会检查以下冲突并安全中止：同项目历史通知不一致、项目名仅大小写不同、去空格后重复、
期次跨项目绑定账号、启用期次绑定停用账号、以及未绑定且无可用默认账号的期次。
遇到错误时先按错误中列出的项目统一旧数据，不要绕过检查。

旧 `/api/v1/admin/activities*`、全局 `/api/v1/admin/auditors*` 和
`/api/v1/admin/projects/status` 已终结；调用方须改用 `/admin/projects/{project_id}/...`
及 `/admin/periods/statuses`。回滚 migration 只能用当前项目配置重建旧冗余字段，无法恢复各历史期次
曾经不同的配置；生产回滚应优先恢复升级前备份。

升级已完成。
服务端版本：1.0.0
插件版本：0.3.0
API 地址统一为：https://api.example.com
内部插件共用一个 XINGTU_SESSION_UPLOAD_TOKEN，无需每人一个 Token。
每个插件包只固化对应的星图账号 ID、领取人名称和分发编号。
插件不再携带管理员 Token，也不能触发工作流，只能上传登录态。
私密配置与源码分离，并已加入 .gitignore。
自动生成 ZIP 与 SHA-256 校验文件。
使用方式：
cp config/xingtu-extension-packages.example.json \
  config/xingtu-extension-packages.json
编辑私密配置，填写共用 Token 和不同账号信息。服务端设置相同 Token：
XINGTU_SESSION_UPLOAD_TOKEN=replace-with-a-generated-random-token
生成所有插件包：
bun scripts/package-xingtu-extension.ts
只生成指定插件包：
bun scripts/package-xingtu-extension.ts --distribution operator-a-main
产物在：
dist/xingtu-session-uploader/
相关文件：
[插件使用说明](/Users/trih/Documents/ROK-PM/dev/lark-utils-exp/browser-extensions/xingtu-session-uploader/README.md)
[私密配置模板](/Users/trih/Documents/ROK-PM/dev/lark-utils-exp/config/xingtu-extension-packages.example.json)
[Bun 打包脚本](/Users/trih/Documents/ROK-PM/dev/lark-utils-exp/scripts/package-xingtu-extension.ts)
[Docker 部署说明](/Users/trih/Documents/ROK-PM/dev/lark-utils-exp/docs/docker-deploy.md)
补充说明：XINGTU_SESSION_ENCRYPTION_KEY_ID=wjm 和插件 Token 无关。它只是数据库登录态加密密钥的版本标识，wjm 作为测试值没有问题；真正负责加密的是对应的 encryption key。
验证结果：Rust 103 项测试、Clippy、格式检查、Bun 打包测试和实际 ZIP 生成测试全部通过。由于没有真实账号清单和共用 Token，我没有生成或覆盖正式分发包，也没有碰原来的未跟踪 ZIP。
