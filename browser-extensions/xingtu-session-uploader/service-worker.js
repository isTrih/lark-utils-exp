import {
  CSRF_COOKIE_NAMES,
  XINGTU_COOKIE_DOMAIN,
  XINGTU_COOKIE_URL,
  buildSessionEndpoint,
  compactString,
  formatError,
  getConfig,
  saveLastStatus,
  sessionUploadAuthorizationHeaders,
} from "./shared.js";

const AUTO_UPLOAD_ALARM = "xingtu-session-auto-upload";
const MIN_AUTO_UPLOAD_GAP_MS = 5 * 60 * 1000;
let lastAutoUploadAt = 0;

chrome.runtime.onInstalled.addListener(() => {
  resetAutoUploadAlarm().catch(console.error);
});

chrome.runtime.onStartup.addListener(() => {
  resetAutoUploadAlarm().catch(console.error);
});

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name !== AUTO_UPLOAD_ALARM) {
    return;
  }

  uploadSession({ reason: "alarm" }).catch(console.error);
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.status !== "complete" || !isXingtuUrl(tab.url)) {
    return;
  }

  uploadSession({ tabId, reason: "tab-complete", auto: true }).catch(console.error);
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  handleMessage(message, sender)
    .then((data) => sendResponse({ ok: true, data }))
    .catch((error) => sendResponse({ ok: false, message: formatError(error) }));
  return true;
});

async function handleMessage(message, sender) {
  switch (message?.type) {
    case "UPLOAD_XINGTU_SESSION":
      return uploadSession({
        tabId: message.tabId ?? sender.tab?.id,
        reason: "manual",
        force: true,
      });
    case "RESET_AUTO_UPLOAD_ALARM":
      return resetAutoUploadAlarm();
    case "GET_RUNTIME_STATUS":
      return {
        lastAutoUploadAt,
      };
    default:
      throw new Error(`未知消息类型：${message?.type ?? "empty"}`);
  }
}

async function resetAutoUploadAlarm() {
  const config = await getConfig();
  await chrome.alarms.clear(AUTO_UPLOAD_ALARM);

  if (!config.autoUploadEnabled) {
    return { enabled: false };
  }

  await chrome.alarms.create(AUTO_UPLOAD_ALARM, {
    delayInMinutes: 1,
    periodInMinutes: Math.max(5, Number(config.uploadIntervalMinutes) || 30),
  });

  return { enabled: true };
}

async function uploadSession(options = {}) {
  const config = await getConfig();

  if (options.auto && !config.autoUploadEnabled) {
    return { skipped: true, reason: "auto-disabled" };
  }

  if (options.auto && Date.now() - lastAutoUploadAt < MIN_AUTO_UPLOAD_GAP_MS) {
    return { skipped: true, reason: "throttled" };
  }

  const collected = await collectSession(options.tabId);
  const payload = {
    xingtu_account_id: compactString(config.xingtuAccountId),
    cookie: collected.cookieHeader,
    csrf_token: collected.csrfToken,
    session_key: compactString(collected.pageState?.sessionKey),
    user_agent: compactString(collected.pageState?.userAgent),
    extra_headers: {},
  };

  if (!payload.xingtu_account_id) {
    throw new Error("缺少星图账号 ID");
  }

  if (!payload.cookie) {
    throw new Error("没有采集到 xingtu.cn cookie，请先在浏览器中登录星图");
  }

  if (!payload.csrf_token) {
    throw new Error("没有采集到 csrf token，请确认星图登录态仍然有效");
  }

  const response = await fetch(buildSessionEndpoint(config.serverBaseUrl), {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...sessionUploadAuthorizationHeaders(),
    },
    body: JSON.stringify(payload),
  });
  const body = await readJsonResponse(response);

  lastAutoUploadAt = Date.now();
  await saveLastStatus({
    ok: true,
    action: "upload-session",
    reason: options.reason ?? "unknown",
    distributionId: config.distributionId,
    recipientName: config.recipientName,
    serverBaseUrl: config.serverBaseUrl,
    xingtuAccountId: payload.xingtu_account_id,
    csrfCookieName: collected.csrfCookieName,
    cookieCount: collected.cookieCount,
    response: body,
  });

  return {
    uploaded: true,
    csrfCookieName: collected.csrfCookieName,
    cookieCount: collected.cookieCount,
    response: body,
  };
}

async function collectSession(tabId) {
  const cookies = mergeCookies([
    ...(await chrome.cookies.getAll({ url: XINGTU_COOKIE_URL })),
    ...(await chrome.cookies.getAll({ domain: XINGTU_COOKIE_DOMAIN })),
    ...(await chrome.cookies.getAll({ domain: "xingtu.cn" })),
  ]);
  const cookieHeader = cookies
    .filter((cookie) => cookie.name && typeof cookie.value === "string")
    .sort((left, right) => left.name.localeCompare(right.name))
    .map((cookie) => `${cookie.name}=${cookie.value}`)
    .join("; ");
  const csrfCookie = findCsrfCookie(cookies);
  const pageState = await collectPageState(tabId);

  return {
    cookieHeader,
    csrfToken: csrfCookie?.value ?? null,
    csrfCookieName: csrfCookie?.name ?? null,
    cookieCount: cookies.length,
    pageState,
  };
}

function mergeCookies(cookies) {
  const byKey = new Map();

  for (const cookie of cookies) {
    byKey.set(`${cookie.domain}:${cookie.path}:${cookie.name}`, cookie);
  }

  return [...byKey.values()];
}

async function collectPageState(tabId) {
  const targetTabId = tabId ?? (await findActiveXingtuTabId());
  if (!targetTabId) {
    return {
      userAgent: navigator.userAgent,
      sessionKey: null,
      pageUrl: null,
    };
  }

  try {
    const response = await chrome.tabs.sendMessage(targetTabId, {
      type: "XINGTU_COLLECT_PAGE_STATE",
    });
    return response?.data ?? null;
  } catch (_error) {
    // 页面脚本可能还没注入完成；cookie/csrf 足够服务端校验时继续上传。
    return {
      userAgent: navigator.userAgent,
      sessionKey: null,
      pageUrl: null,
    };
  }
}

async function findActiveXingtuTabId() {
  const tabs = await chrome.tabs.query({
    active: true,
    currentWindow: true,
  });
  const tab = tabs.find((item) => isXingtuUrl(item.url));
  return tab?.id ?? null;
}

function findCsrfCookie(cookies) {
  for (const name of CSRF_COOKIE_NAMES) {
    const cookie = cookies.find((item) => item.name === name && item.value);
    if (cookie) {
      return cookie;
    }
  }

  return cookies.find((item) => /csrf/i.test(item.name) && item.value) ?? null;
}

async function readJsonResponse(response) {
  const text = await response.text();
  let body = {};

  try {
    body = text ? JSON.parse(text) : {};
  } catch (_error) {
    body = { message: text };
  }

  if (!response.ok || body.ok === false) {
    throw new Error(body.message || `HTTP ${response.status}`);
  }

  return body;
}

function isXingtuUrl(url) {
  if (!url) {
    return false;
  }

  try {
    const parsed = new URL(url);
    return parsed.hostname === "xingtu.cn" || parsed.hostname.endsWith(".xingtu.cn");
  } catch (_error) {
    return false;
  }
}
