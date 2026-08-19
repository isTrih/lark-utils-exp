const PROTECTION_SCHEME = "aes-256-gcm+zstd";
const PROTECTION_VERSION = 1;
const AAD = new TextEncoder().encode("lark-utils-exp:data:v1");

interface ProtectedEnvelope {
  protected: true;
  data: {
    version: 1;
    scheme: typeof PROTECTION_SCHEME;
    key_id: string;
    nonce: string;
    ciphertext: string;
  };
}

function decodeStandardBase64(value: string, fieldName: string): Buffer {
  if (
    value.length === 0 ||
    value.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(value)
  ) {
    throw new Error(`${fieldName} 不是标准 Base64`);
  }

  const decoded = Buffer.from(value, "base64");
  if (decoded.toString("base64") !== value) {
    throw new Error(`${fieldName} 不是规范的标准 Base64`);
  }
  return decoded;
}

function decodeBase64Url(value: string, fieldName: string): Buffer {
  if (value.length === 0 || !/^[A-Za-z0-9_-]+$/.test(value)) {
    throw new Error(`${fieldName} 不是无 padding 的 Base64URL`);
  }

  const decoded = Buffer.from(value, "base64url");
  if (decoded.toString("base64url") !== value) {
    throw new Error(`${fieldName} 不是规范的 Base64URL`);
  }
  return decoded;
}

function parseEnvelope(input: string): ProtectedEnvelope {
  let value: unknown;
  try {
    value = JSON.parse(input);
  } catch (error) {
    throw new Error(`stdin 不是合法 JSON：${String(error)}`);
  }

  if (typeof value !== "object" || value === null) {
    throw new Error("保护信封必须是 JSON object");
  }

  const envelope = value as Partial<ProtectedEnvelope>;
  if (envelope.protected !== true || typeof envelope.data !== "object") {
    throw new Error("保护信封缺少 protected=true 或 data");
  }

  const data = envelope.data as Partial<ProtectedEnvelope["data"]>;
  if (data.version !== PROTECTION_VERSION) {
    throw new Error(`不支持的信封版本：${String(data.version)}`);
  }
  if (data.scheme !== PROTECTION_SCHEME) {
    throw new Error(`不支持的保护方案：${String(data.scheme)}`);
  }
  if (typeof data.key_id !== "string" || data.key_id.trim().length === 0) {
    throw new Error("保护信封缺少 key_id");
  }
  if (typeof data.nonce !== "string" || typeof data.ciphertext !== "string") {
    throw new Error("保护信封缺少 nonce 或 ciphertext");
  }

  return value as ProtectedEnvelope;
}

async function main(): Promise<void> {
  const keyBase64 = process.env.API_DATA_ENCRYPTION_KEY?.trim();
  if (!keyBase64) {
    throw new Error("缺少环境变量 API_DATA_ENCRYPTION_KEY");
  }

  const keyBytes = decodeStandardBase64(
    keyBase64,
    "API_DATA_ENCRYPTION_KEY",
  );
  if (keyBytes.byteLength !== 32) {
    throw new Error("API_DATA_ENCRYPTION_KEY 解码后必须恰好为 32 字节");
  }

  const input = (await Bun.stdin.text()).trim();
  if (!input) {
    throw new Error("stdin 中没有保护信封 JSON");
  }
  const envelope = parseEnvelope(input);

  const expectedKeyId =
    process.env.API_DATA_ENCRYPTION_KEY_ID?.trim() || "primary";
  if (envelope.data.key_id !== expectedKeyId) {
    throw new Error(
      `信封 key_id=${envelope.data.key_id} 与期望值 ${expectedKeyId} 不一致`,
    );
  }

  const nonce = decodeBase64Url(envelope.data.nonce, "data.nonce");
  if (nonce.byteLength !== 12) {
    throw new Error("data.nonce 解码后必须为 12 字节");
  }
  const ciphertext = decodeBase64Url(
    envelope.data.ciphertext,
    "data.ciphertext",
  );
  if (ciphertext.byteLength < 16) {
    throw new Error("data.ciphertext 长度不足，缺少 AES-GCM 认证标签");
  }

  const key = await crypto.subtle.importKey(
    "raw",
    keyBytes,
    { name: "AES-GCM" },
    false,
    ["decrypt"],
  );
  const compressed = await crypto.subtle.decrypt(
    {
      name: "AES-GCM",
      iv: nonce,
      additionalData: AAD,
      tagLength: 128,
    },
    key,
    ciphertext,
  );
  const jsonBytes = Bun.zstdDecompressSync(new Uint8Array(compressed));
  const jsonText = new TextDecoder("utf-8", { fatal: true }).decode(jsonBytes);

  let payload: unknown;
  try {
    payload = JSON.parse(jsonText);
  } catch (error) {
    throw new Error(`解密后的内容不是合法 JSON：${String(error)}`);
  }

  console.log(JSON.stringify(payload, null, 2));
}

main().catch((error) => {
  console.error(`解密失败：${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
