import {
  chmod,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

type PackageDefaults = {
  autoUploadEnabled?: boolean;
  uploadIntervalMinutes?: number;
};

type PackageEntry = PackageDefaults & {
  distributionId: string;
  recipientName: string;
  xingtuAccountId: string;
  enabled?: boolean;
};

export type ExtensionPackageConfig = {
  version: number;
  serverBaseUrl: string;
  sessionUploadToken: string;
  defaults?: PackageDefaults;
  packages: PackageEntry[];
};

type ValidatedConfig = ExtensionPackageConfig & {
  serverBaseUrl: string;
  sessionUploadToken: string;
  packages: PackageEntry[];
};

const ROOT = resolve(import.meta.dir, "..");
const EXTENSION_SOURCE = join(ROOT, "browser-extensions/xingtu-session-uploader");
const DEFAULT_CONFIG_PATH = join(ROOT, "config/xingtu-extension-packages.json");
const DEFAULT_OUTPUT_DIR = join(ROOT, "dist/xingtu-session-uploader");
const STATIC_FILES = [
  "content-script.js",
  "popup.css",
  "popup.html",
  "popup.js",
  "service-worker.js",
  "shared.js",
] as const;

function requiredArgument(name: string): string | undefined {
  const index = Bun.argv.indexOf(name);
  return index >= 0 ? Bun.argv[index + 1] : undefined;
}

function optionalString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function normalizedInterval(value: unknown, fallback = 30): number {
  const interval = Number(value ?? fallback);
  if (!Number.isInteger(interval) || interval < 5 || interval > 1_440) {
    throw new Error("uploadIntervalMinutes 必须是 5..1440 的整数");
  }
  return interval;
}

export function validatePackageConfig(raw: unknown): ValidatedConfig {
  if (!raw || typeof raw !== "object") throw new Error("配置文件必须是 JSON 对象");
  const config = raw as Partial<ExtensionPackageConfig>;
  if (config.version !== 1) throw new Error("配置文件 version 必须为 1");

  const serverBaseUrl = optionalString(config.serverBaseUrl).replace(/\/+$/, "");
  const parsedServerUrl = new URL(serverBaseUrl);
  if (parsedServerUrl.protocol !== "https:") throw new Error("serverBaseUrl 必须使用 HTTPS");
  if (parsedServerUrl.pathname !== "/" || parsedServerUrl.search || parsedServerUrl.hash) {
    throw new Error("serverBaseUrl 只能包含协议、域名和可选端口");
  }

  const sessionUploadToken = optionalString(config.sessionUploadToken);
  if (sessionUploadToken.length < 32 || /replace|example|change-me/i.test(sessionUploadToken)) {
    throw new Error("sessionUploadToken 必须替换为至少 32 字符的真实随机 Token");
  }
  if (!Array.isArray(config.packages) || config.packages.length === 0) {
    throw new Error("packages 至少需要一项");
  }

  const defaults = config.defaults ?? {};
  const distributionIds = new Set<string>();
  const packages = config.packages.map((entry, index) => {
    if (!entry || typeof entry !== "object") throw new Error(`packages[${index}] 必须是对象`);
    const distributionId = optionalString(entry.distributionId);
    if (!/^[a-z0-9][a-z0-9._-]{1,63}$/i.test(distributionId)) {
      throw new Error(`packages[${index}].distributionId 格式无效`);
    }
    if (distributionIds.has(distributionId)) {
      throw new Error(`distributionId 重复：${distributionId}`);
    }
    distributionIds.add(distributionId);

    const recipientName = optionalString(entry.recipientName);
    if (!recipientName || recipientName.length > 40) {
      throw new Error(`packages[${index}].recipientName 必须为 1..40 个字符`);
    }
    const xingtuAccountId = optionalString(entry.xingtuAccountId);
    if (!xingtuAccountId || /replace|example/i.test(xingtuAccountId)) {
      throw new Error(`packages[${index}].xingtuAccountId 未配置`);
    }

    return {
      distributionId,
      recipientName,
      xingtuAccountId,
      enabled: entry.enabled !== false,
      autoUploadEnabled: entry.autoUploadEnabled ?? defaults.autoUploadEnabled ?? true,
      uploadIntervalMinutes: normalizedInterval(
        entry.uploadIntervalMinutes,
        defaults.uploadIntervalMinutes,
      ),
    };
  });

  return {
    version: 1,
    serverBaseUrl,
    sessionUploadToken,
    defaults,
    packages,
  };
}

export function createInstanceConfigModule(
  config: ValidatedConfig,
  entry: PackageEntry,
): string {
  return `// 由 Bun 打包脚本生成。此文件包含内部上传凭据，请勿公开或提交到 Git。\nexport const INSTANCE_CONFIG = Object.freeze(${JSON.stringify(
    {
      distributionId: entry.distributionId,
      recipientName: entry.recipientName,
      serverBaseUrl: config.serverBaseUrl,
      xingtuAccountId: entry.xingtuAccountId,
      sessionUploadToken: config.sessionUploadToken,
      autoUploadEnabled: entry.autoUploadEnabled !== false,
      uploadIntervalMinutes: normalizedInterval(entry.uploadIntervalMinutes),
    },
    null,
    2,
  )});\n`;
}

export function buildPackageManifest(
  sourceManifest: Record<string, unknown>,
  config: ValidatedConfig,
  entry: PackageEntry,
): Record<string, unknown> {
  const origin = new URL(config.serverBaseUrl).origin;
  return {
    ...sourceManifest,
    name: `星图登录态同步助手 · ${entry.recipientName}`,
    description: `为 ${entry.recipientName} 的星图账号自动同步登录态。`,
    host_permissions: [
      "https://www.xingtu.cn/*",
      "https://*.xingtu.cn/*",
      `${origin}/*`,
    ],
  };
}

async function packageOne(
  config: ValidatedConfig,
  entry: PackageEntry,
  outputDir: string,
): Promise<void> {
  const temporaryRoot = await mkdtemp(join(tmpdir(), "xingtu-session-uploader-"));
  const stage = join(temporaryRoot, "extension");
  const zipPath = join(outputDir, `xingtu-session-uploader-${entry.distributionId}.zip`);
  const checksumPath = `${zipPath}.sha256`;

  try {
    await mkdir(stage, { recursive: true });
    await Promise.all(
      STATIC_FILES.map((file) => cp(join(EXTENSION_SOURCE, file), join(stage, file))),
    );
    const sourceManifest = JSON.parse(
      await readFile(join(EXTENSION_SOURCE, "manifest.json"), "utf8"),
    ) as Record<string, unknown>;
    await writeFile(
      join(stage, "manifest.json"),
      `${JSON.stringify(buildPackageManifest(sourceManifest, config, entry), null, 2)}\n`,
      { mode: 0o600 },
    );
    await writeFile(join(stage, "instance-config.js"), createInstanceConfigModule(config, entry), {
      mode: 0o600,
    });

    await mkdir(outputDir, { recursive: true });
    await rm(zipPath, { force: true });
    await rm(checksumPath, { force: true });
    const archive = Bun.spawn(["zip", "-q", "-r", zipPath, "."], {
      cwd: stage,
      stdout: "inherit",
      stderr: "inherit",
    });
    const exitCode = await archive.exited;
    if (exitCode !== 0) throw new Error(`zip 打包失败，退出码：${exitCode}`);

    const hasher = new Bun.CryptoHasher("sha256");
    hasher.update(await Bun.file(zipPath).arrayBuffer());
    const digest = hasher.digest("hex");
    await writeFile(checksumPath, `${digest}  ${basename(zipPath)}\n`, { mode: 0o600 });
    await chmod(zipPath, 0o600);
    process.stdout.write(`已生成 ${zipPath}\nSHA-256 ${digest}\n`);
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true });
  }
}

async function main(): Promise<void> {
  const configPath = resolve(requiredArgument("--config") ?? DEFAULT_CONFIG_PATH);
  const outputDir = resolve(requiredArgument("--output") ?? DEFAULT_OUTPUT_DIR);
  const selectedDistribution = optionalString(requiredArgument("--distribution"));
  const config = validatePackageConfig(JSON.parse(await readFile(configPath, "utf8")));
  const packages = config.packages.filter(
    (entry) => entry.enabled !== false && (!selectedDistribution || entry.distributionId === selectedDistribution),
  );
  if (packages.length === 0) {
    throw new Error(
      selectedDistribution
        ? `没有找到已启用的分发配置：${selectedDistribution}`
        : "没有已启用的分发配置",
    );
  }
  for (const entry of packages) await packageOne(config, entry, outputDir);
}

if (import.meta.main) {
  await main().catch((error) => {
    process.stderr.write(`打包失败：${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
