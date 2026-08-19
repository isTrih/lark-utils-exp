# 开源安全审计记录

审计日期：2026-08-19

## 范围

本次检查覆盖：

- Git 当前跟踪文件；
- 未跟踪且未被 `.gitignore` 排除的文件；
- `.gitignore` 与 `.dockerignore`；
- 常见密钥、私钥、Token、Cookie、数据库备份和插件包模式；
- Git 历史中的敏感文件名和真实业务数据文件；
- 文档、示例、测试入口和浏览器插件中的组织专属配置。

扫描输出只记录文件、提交和风险类型，不记录疑似密钥原文。

## 历史风险与处理结果

### 1. 历史提交曾包含 `.env`

Git 历史显示 `.env` 曾存在于提交 `170f175`，随后在 `2a13187` 的历史范围内发生变更。历史文件包含真实形态的 `LARK_APP_ID`、`LARK_APP_SECRET` 和 `LARK_BASE_URL` 配置。

处理结果：

1. 当前脱敏工作树已重建为新的单一根提交，旧 `.env` 不再存在于仓库可达历史。
2. 旧引用和 reflog 已清除，并对本地对象库执行立即回收。
3. 旧历史在仓库外保留一份权限为 `0600` 的私密 bundle，仅用于紧急恢复，不得公开。
4. 飞书 App Secret 的轮换必须由维护者在飞书开放平台完成；历史清理不能使曾暴露的 Secret 恢复安全。

任何进入过 Git 的秘密都应按已泄漏处理，即使仓库从未公开。

### 2. 历史提交包含真实业务导出

`sheet_data.csv` 和 `sheet_data.json` 自提交 `170f175` 进入历史，其中包含作者名称、作者 UID、内容标题、视频 ID、播放互动指标和业务链接等真实业务数据。

当前公开根提交不包含这两个文件，旧引用和本地不可达对象已经清除。本地副本仅保留在被忽略的 `private-data/exports/`。

### 3. 历史提交包含真实活动配置

`configs/rok-2026-07-p14.json` 自提交 `2a13187` 进入历史，包含真实星图账号、任务 ID、飞书多维表链接和表 ID。当前工作树已删除跟踪文件，并将本地副本移动到被忽略的 `private-data/configs/`。

当前公开根提交不包含该文件，旧引用和本地不可达对象已经清除。本地副本仅保留在被忽略的 `private-data/configs/`。

### 4. 已添加开源许可证

根目录已添加标准 MIT `LICENSE`，`Cargo.toml` 同步声明 `license = "MIT"`，README 说明了代码许可与第三方服务、素材及业务数据权利之间的边界。

## 当前工作树整改

已完成：

- CORS 域名从组织域名硬编码改为 `CORS_DOMAIN` 环境变量。
- `.gitignore` 增加环境文件、私有配置、业务导出、备份、数据库、证书、日志、构建产物和插件 ZIP 规则。
- `.dockerignore` 同步排除私有数据、备份、证书和压缩包。
- 真实业务导出和真实活动配置从当前跟踪树移除，本地副本保留在 `private-data/`。
- `.env.example`、插件模板、测试入口、API 文档和配置示例改为占位数据。
- 项目汇报卡片模板改为环境变量，项目审核/登录异常配置继续由项目数据库管理。
- 移除调试登录态的默认星图账号 ID，配置 Cookie 和 CSRF Token 时必须显式设置账号 ID。
- CI 同时覆盖 `main` 与 `master` 分支。
- 根目录新增公开部署 README 和明确的安全发布门禁。
- 根目录新增 MIT `LICENSE`，Rust package metadata 和 README 已同步声明许可证。

当前被忽略的本地敏感/生成内容包括 `.env`、`config/xingtu-extension-packages.json`、`private-data/`、`dist/`、`target/` 和依赖目录。

## 不应直接修改的历史 migration

部分已经执行过的 migration 中仍存在历史卡片模板 ID 和业务表 ID。它们不是认证密钥，但属于组织专属标识。

生产数据库依赖 SQLx migration 校验和，不能为了公开仓库直接修改已经执行过的 migration。若维护者要求连这些历史标识也完全不公开，建议为公开版创建经过审阅的全新 baseline migration 和全新 Git 仓库，而不是改写仍服务于生产数据库的 migration 文件。

## 本次历史清理方式

本次采用“当前脱敏快照创建单一根提交”的方式，不保留旧提交的父链：

1. 在仓库外创建包含清理前全部引用的私密 Git bundle，并将权限设为 `0600`。
2. 将当前工作树和删除记录写入新的 root tree，再以无父提交方式创建新的 `master` 根提交。
3. 删除仍指向旧提交的内部 refs，立即过期全部 reflog，并执行 `git gc --prune=now --aggressive`。
4. 验证所有 refs 只有一个提交，目标敏感路径没有历史记录，且 `git fsck --full --unreachable` 不再报告旧对象。

仓库当前未配置远端，因此本次没有执行 push。若旧历史此前已经推送到任何远端，必须删除或替换该远端，并要求已有克隆者重新克隆；仅覆盖默认分支不能保证其他 refs、fork 或缓存中的旧对象消失。

## 公开前复核清单

- [ ] 历史飞书 App Secret 已轮换。
- [x] 已将当前脱敏快照重建为单一根提交并清除本地旧对象。
- [ ] 首次推送后在独立目录重新克隆并确认敏感文件不存在。
- [x] 已对当前公开快照重新运行敏感模式扫描，未发现真实凭据。
- [x] 已添加 MIT `LICENSE`，并同步更新 package metadata 与 README。
- [ ] 托管平台已启用 Secret Scanning 和 Push Protection。
- [ ] Docker 镜像构建上下文不包含 `.env`、`private-data/`、备份或插件 ZIP。
- [ ] 文档、Issue 模板和示例数据只包含占位值。
- [ ] `cargo fmt`、`cargo check`、`cargo test`、`cargo clippy`、Bun 测试和 `git diff --check` 全部通过。
