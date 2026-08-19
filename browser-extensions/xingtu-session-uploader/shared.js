import { INSTANCE_CONFIG } from "./instance-config.js";

const DEFAULT_SERVER_BASE_URL = "https://api.example.com";

export const DEFAULT_CONFIG = Object.freeze({
  distributionId: compactString(INSTANCE_CONFIG.distributionId) ?? "unconfigured",
  recipientName: compactString(INSTANCE_CONFIG.recipientName) ?? "未配置",
  serverBaseUrl: normalizeServerBaseUrl(INSTANCE_CONFIG.serverBaseUrl),
  xingtuAccountId: compactString(INSTANCE_CONFIG.xingtuAccountId) ?? "",
  autoUploadEnabled: INSTANCE_CONFIG.autoUploadEnabled !== false,
  uploadIntervalMinutes: Math.max(5, Number(INSTANCE_CONFIG.uploadIntervalMinutes) || 30),
});

export const STORAGE_KEYS = {
  config: "xingtu_session_uploader_config",
  lastStatus: "xingtu_session_uploader_last_status",
};

export const XINGTU_COOKIE_URL = "https://www.xingtu.cn";
export const XINGTU_COOKIE_DOMAIN = ".xingtu.cn";
export const CSRF_COOKIE_NAMES = ["passport_csrf_token", "csrf_token", "tt_csrf_token"];
export const SESSION_KEY_CANDIDATES = [
  "XINGTU_SESSION_KEY",
  "session_key",
  "sessionKey",
  "__xingtu_session_key__",
];

export async function getConfig() {
  const result = await chrome.storage.sync.get(STORAGE_KEYS.config);
  const saved = result[STORAGE_KEYS.config] ?? {};
  return {
    ...DEFAULT_CONFIG,
    autoUploadEnabled:
      typeof saved.autoUploadEnabled === "boolean"
        ? saved.autoUploadEnabled
        : DEFAULT_CONFIG.autoUploadEnabled,
    uploadIntervalMinutes: Math.max(
      5,
      Number(saved.uploadIntervalMinutes) || DEFAULT_CONFIG.uploadIntervalMinutes,
    ),
  };
}

export async function saveConfig(config) {
  await chrome.storage.sync.set({
    [STORAGE_KEYS.config]: {
      autoUploadEnabled: config.autoUploadEnabled !== false,
      uploadIntervalMinutes: Math.max(
        5,
        Number(config.uploadIntervalMinutes) || DEFAULT_CONFIG.uploadIntervalMinutes,
      ),
    },
  });
}

export async function getLastStatus() {
  const result = await chrome.storage.local.get(STORAGE_KEYS.lastStatus);
  return result[STORAGE_KEYS.lastStatus] ?? null;
}

export async function saveLastStatus(status) {
  await chrome.storage.local.set({
    [STORAGE_KEYS.lastStatus]: {
      ...status,
      updatedAt: new Date().toISOString(),
    },
  });
}

export function sessionUploadAuthorizationHeaders() {
  const token = String(INSTANCE_CONFIG.sessionUploadToken ?? "").trim();
  if (!token) {
    throw new Error("当前插件包未配置登录态上传 Token，请使用打包脚本重新生成");
  }
  return { Authorization: `Bearer ${token}` };
}

export function normalizeServerBaseUrl(value) {
  return trimTrailingSlash(value || DEFAULT_SERVER_BASE_URL);
}

export function buildSessionEndpoint(serverBaseUrl) {
  return `${normalizeServerBaseUrl(serverBaseUrl)}/api/v1/xingtu/sessions`;
}

export function trimTrailingSlash(value) {
  return String(value).trim().replace(/\/+$/, "");
}

export function compactString(value) {
  const text = String(value ?? "").trim();
  return text.length > 0 ? text : null;
}

export function formatError(error) {
  if (!error) {
    return "未知错误";
  }

  if (typeof error === "string") {
    return error;
  }

  return error.message || String(error);
}
