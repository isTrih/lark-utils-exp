# 星图登录态同步助手

Chrome/Edge Manifest V3 内部插件。在浏览器登录 `xingtu.cn` 后，自动采集 Cookie、CSRF
Token 和可用的 `session_key`，上传到星图工作流服务端。

## 0.3.0 结构

- 默认服务地址为 `https://api.example.com`。
- 插件只负责登录态上传，不再携带管理员 Token，也不再触发工作流。
- 所有内部插件包共用一个 `XINGTU_SESSION_UPLOAD_TOKEN`。
- 每个 ZIP 固定一个分发对象和星图账号 ID，弹窗中只读，避免使用者选错账号。
- 真实 Token、人员和账号清单只保存在 Git 忽略的私有 JSON 配置中。
- 每个包自动生成 SHA-256 校验文件，方便确认分发文件未被替换。

最终 ZIP 内必然包含上传 Token。它是内部凭据而不是不可提取的秘密，必须通过可信渠道分发；
该 Token 在服务端只能访问 `POST /api/v1/xingtu/sessions`，不能调用工作流和管理接口。

## 一次性配置

复制模板：

```bash
cp config/xingtu-extension-packages.example.json config/xingtu-extension-packages.json
```

生成一个共享随机 Token：

```bash
bun -e 'console.log(Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64url"))'
```

将同一个值配置到：

1. 服务端环境变量 `XINGTU_SESSION_UPLOAD_TOKEN`。
2. 私有配置文件的 `sessionUploadToken`。

不要把它设置成 `XINGTU_SESSION_ENCRYPTION_KEY` 或 `MUTATION_API_TOKEN`。上传 Token 会写入
内部插件包，而登录态数据库加密密钥与管理员 Token 必须只留在服务端。

内部打包前端如需读取当前共享上传 Token，可使用管理员 Token 调用：

```http
GET /api/v1/admin/xingtu/session-upload-token
Authorization: Bearer <MUTATION_API_TOKEN>
```

响应不会被缓存，也不会返回 `XINGTU_SESSION_ENCRYPTION_KEY`。

私有配置结构：

```json
{
  "version": 1,
  "serverBaseUrl": "https://api.example.com",
  "sessionUploadToken": "共享随机上传Token",
  "defaults": {
    "autoUploadEnabled": true,
    "uploadIntervalMinutes": 30
  },
  "packages": [
    {
      "distributionId": "operator-a-main",
      "recipientName": "运营同事 A",
      "xingtuAccountId": "replace-with-xingtu-account-id",
      "enabled": true
    }
  ]
}
```

`distributionId` 只能使用字母、数字、点、下划线和短横线；账号 ID、分发编号不能使用示例占位值。
上传间隔限制为 5–1440 分钟。

## 生成分发包

生成配置中全部 `enabled=true` 的包：

```bash
bun scripts/package-xingtu-extension.ts
```

只生成一个指定包：

```bash
bun scripts/package-xingtu-extension.ts --distribution operator-a-main
```

指定配置文件或输出目录：

```bash
bun scripts/package-xingtu-extension.ts \
  --config /secure/path/xingtu-extension-packages.json \
  --output /secure/output
```

默认输出位于 Git 忽略目录：

```text
dist/xingtu-session-uploader/
├── xingtu-session-uploader-operator-a-main.zip
└── xingtu-session-uploader-operator-a-main.zip.sha256
```

打包脚本会在临时目录生成只包含当前分发对象的 `instance-config.js`，不会把整份账号清单
装入 ZIP。源目录中的 `instance-config.js` 只是无密钥占位配置，不能用于正式上传。

## 安装与使用

1. 通过可信渠道把对应 ZIP 和 `.sha256` 文件交给使用者。
2. 校验 SHA-256 后解压 ZIP。
3. 打开 Chrome/Edge 扩展管理页并启用开发者模式。
4. 点击“加载已解压的扩展程序”，选择解压目录。
5. 登录 `https://www.xingtu.cn`。
6. 插件会按配置周期自动上传，也可点击“采集并上传”。

弹窗只能修改是否自动上传及上传间隔。分发对象、服务地址、星图账号 ID 均由包固定，修改时
应更新私有配置并重新生成 ZIP。

## Token 轮换

共享 Token 泄漏或需要定期轮换时：

1. 生成新的随机 Token。
2. 更新服务端 `XINGTU_SESSION_UPLOAD_TOKEN` 并重启服务。
3. 更新私有打包配置。
4. 重新生成并分发全部启用的插件包。
5. 删除旧 ZIP；旧包会立即失去上传权限。

不要提交 `config/xingtu-extension-packages.json`、生成后的 ZIP，或任何真实 Cookie、CSRF
Token、上传 Token。
