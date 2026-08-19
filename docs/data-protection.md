# API 数据保护与解密

本文档描述普通 `/api/v1/**` JSON API 的传输压缩和可选应用层保护。应用层加密是为可控的服务端客户端准备的增强能力，不代替 HTTPS，也不提供用户身份认证。

## 两层保护

### 传输层：HTTPS 和 HTTP 压缩

生产请求仍必须使用 HTTPS。TLS 负责服务端身份校验、传输加密、完整性和防中间人攻击。应用层 AES-GCM 不保护 URL、HTTP 方法、状态码、大部分响应头和流量模式，也不提供 TLS 的服务端身份认证与前向保密性。

客户端可以通过标准 `Accept-Encoding` 协商 `gzip`、`br` 或 `zstd` HTTP 响应压缩。例如 `curl --compressed` 会发送它支持的算法并自动解压 `Content-Encoding`。HTTP 压缩只节省带宽，不是安全机制。

### 应用层：可选的 AES-256-GCM + zstd

受信任的服务端客户端可在 `/api/v1/**` 请求中显式加入：

```http
X-Data-Protection: aes-256-gcm+zstd
```

服务端会保留原 JSON 响应的字节内容，依次执行：

1. zstd level 3 压缩原 JSON 字节。
2. 使用 AES-256-GCM、96-bit 随机 nonce 和固定 AAD `lark-utils-exp:data:v1` 加密压缩结果。
3. 将 nonce 和“密文 + GCM tag”编码为无 padding 的 Base64URL，放入 JSON 信封。

受保护响应使用：

```http
X-Data-Protection: aes-256-gcm+zstd
Content-Type: application/vnd.lark-utils-exp.protected+json
Cache-Control: no-store
Vary: X-Data-Protection
```

保护信封的自定义 Content-Type 不在 HTTP Compression 白名单中，因此已经 zstd 后加密的密文不会再被 HTTP 层二次压缩。

## 兼容边界

- 没有 `X-Data-Protection` 请求头时，现有 API 的状态码、Content-Type 和 JSON 响应保持不变，旧客户端无需升级。
- 请求头名不区分大小写，值必须是完整的 `aes-256-gcm+zstd`。存在但不支持的值返回 HTTP 400，不调用业务处理器。
- 客户端明确请求保护，但服务端未配置可用密钥时返回 HTTP 503，不调用业务处理器，也不降级返回原明文。
- 应用层保护只处理 `/api/v1/**` JSON 响应，不用于请求体。`/health`、OpenAPI 和 Swagger UI 不使用该信封。
- 飞书数据同步插件协议 `/api/data-sync/table-meta` 和 `/api/data-sync/records` 永远保持飞书规定的响应结构，不使用应用层信封。它们的 `DATA_SYNC_SECRET_KEY` 请求签名与本机制互相独立。

## 服务端配置

| 环境变量 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `API_DATA_ENCRYPTION_KEY` | 否 | 无 | 标准 Base64 编码的 32 字节随机密钥，不是 Base64URL |
| `API_DATA_ENCRYPTION_KEY_ID` | 否 | `primary` | 写入信封的密钥标识，便于客户端选择密钥和轮换 |

`API_DATA_ENCRYPTION_KEY` 未配置时不影响普通明文 API；只有显式请求保护的调用会得到 503。如果变量存在但不是规范标准 Base64，或解码后不是恰好 32 字节，服务必须拒绝启动。

使用 Bun 1.2+ 生成 32 字节随机密钥并输出标准 Base64：

```bash
bun -e 'console.log(Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64"))'
```

命令会把密钥打印到终端；生产环境应立即将它收录到密钥管理系统，不要写入代码、配置 JSON、Dockerfile、镜像或 Git 历史。

## 请求与信封

保存一次受保护响应：

```bash
curl --compressed --fail-with-body --silent --show-error \
  -D response.headers \
  -o response.json \
  -H 'X-Data-Protection: aes-256-gcm+zstd' \
  'https://example.internal/api/v1/queries/videos?limit=10'
```

`response.json` 格式如下：

```json
{
  "protected": true,
  "data": {
    "version": 1,
    "scheme": "aes-256-gcm+zstd",
    "key_id": "primary",
    "nonce": "Base64URL-without-padding",
    "ciphertext": "Base64URL-without-padding"
  }
}
```

| 字段 | 说明 |
| --- | --- |
| `protected` | 固定为 `true` |
| `data.version` | 协议版本，当前固定为 `1` |
| `data.scheme` | 固定为 `aes-256-gcm+zstd` |
| `data.key_id` | 服务端当前密钥 ID，不是密钥本身 |
| `data.nonce` | 12 字节 AES-GCM nonce，无 padding Base64URL |
| `data.ciphertext` | AES-GCM 密文与 16 字节认证标签，无 padding Base64URL |

## Bun 解密

仓库提供了无第三方依赖的完整脚本 [scripts/decrypt-data.ts](../scripts/decrypt-data.ts)。它从 stdin 读取信封，使用 `API_DATA_ENCRYPTION_KEY` 解密，zstd 解压后执行 `JSON.parse` 并输出格式化 JSON：

```bash
API_DATA_ENCRYPTION_KEY='replace-with-32-byte-standard-base64-key' \
API_DATA_ENCRYPTION_KEY_ID='primary' \
bun scripts/decrypt-data.ts < response.json
```

下面是与协议等价的 Bun 1.2+ 最小完整解密程序：

```ts
const scheme = "aes-256-gcm+zstd";
const aad = new TextEncoder().encode("lark-utils-exp:data:v1");
const keyBase64 = process.env.API_DATA_ENCRYPTION_KEY?.trim();
if (!keyBase64) throw new Error("missing API_DATA_ENCRYPTION_KEY");

const keyBytes = Buffer.from(keyBase64, "base64");
if (keyBytes.length !== 32 || keyBytes.toString("base64") !== keyBase64) {
  throw new Error("API_DATA_ENCRYPTION_KEY must be canonical Base64 for 32 bytes");
}

const envelope = JSON.parse(await Bun.stdin.text());
if (envelope?.protected !== true) throw new Error("not a protected envelope");
if (envelope.data?.version !== 1) throw new Error("unsupported version");
if (envelope.data?.scheme !== scheme) throw new Error("unsupported scheme");

const expectedKeyId = process.env.API_DATA_ENCRYPTION_KEY_ID?.trim() || "primary";
if (envelope.data.key_id !== expectedKeyId) {
  throw new Error(`unexpected key_id: ${envelope.data.key_id}`);
}

function fromBase64Url(value: string): Buffer {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("invalid Base64URL");
  const bytes = Buffer.from(value, "base64url");
  if (bytes.toString("base64url") !== value) throw new Error("non-canonical Base64URL");
  return bytes;
}

const nonce = fromBase64Url(envelope.data.nonce);
const ciphertext = fromBase64Url(envelope.data.ciphertext);
if (nonce.length !== 12) throw new Error("nonce must be 12 bytes");

const key = await crypto.subtle.importKey(
  "raw",
  keyBytes,
  { name: "AES-GCM" },
  false,
  ["decrypt"],
);
const compressed = await crypto.subtle.decrypt(
  { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
  key,
  ciphertext,
);
const jsonBytes = Bun.zstdDecompressSync(new Uint8Array(compressed));
const payload = JSON.parse(new TextDecoder().decode(jsonBytes));
console.log(JSON.stringify(payload, null, 2));
```

客户端不应在看到非 2xx 状态时直接假设响应为明文。应先检查响应头和 Content-Type：如果返回了保护信封，先解密出原错误 JSON，再根据 HTTP 状态处理业务错误。客户端既然明确请求保护，就必须校验响应的 `X-Data-Protection`、Content-Type、`protected`、`version`、`scheme` 和 `key_id`；任意一项不符都应拒绝处理，不能静默当成明文，以防降级。

## 密钥分发和轮换

- 通过云密钥管理、容器密钥、CI/CD 受保护变量或内网配置中心向服务端和受信任客户端分发密钥。禁止通过 IM、普通邮件、日志或命令行参数传递真实密钥。
- `key_id` 只是标识，不是密钥，不得把 Base64 密钥本身放入 `key_id`。客户端应根据 `key_id` 从自己的密钥环中选择密钥。
- 轮换时先生成新密钥和新 ID，将新密钥预发到客户端密钥环；再切换服务端环境变量并重启；观察所有实例和客户端后，最后撤销旧密钥。滚动发布期间可能同时出现新旧 `key_id`，客户端必须在过渡期保留两把密钥。
- 发现泄漏时立即生成全新密钥和 ID，停用旧密钥，并审计该 ID 相关的访问日志。

## 浏览器和错误处理

不要把 `API_DATA_ENCRYPTION_KEY` 放入浏览器代码、前端环境变量、LocalStorage、IndexedDB、Cookie、浏览器扩展或可下载的静态资源。向浏览器分发对称密钥等同于向用户分发该密钥。浏览器应继续通过 HTTPS 调用普通明文 JSON API，或调用由受信任后端代理完成解密的 BFF。

解密客户端必须 fail closed：Base64/Base64URL 非法、nonce 长度错误、未知版本或方案、未知 `key_id`、GCM tag 校验失败、zstd 解压失败或 JSON 解析失败时，都应停止处理、记录不含密钥/密文/原文的诊断信息，并按调用失败处理。不得在失败后尝试将信封当成业务 JSON，也不得在日志中打印密钥、Cookie、CSRF Token 或解密后的敏感数据。
