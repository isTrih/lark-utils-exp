import {
  DEFAULT_CONFIG,
  formatError,
  getConfig,
  getLastStatus,
  saveConfig,
} from "./shared.js";

const elements = {
  statusText: document.querySelector("#statusText"),
  distributionId: document.querySelector("#distributionId"),
  recipientName: document.querySelector("#recipientName"),
  serverBaseUrl: document.querySelector("#serverBaseUrl"),
  xingtuAccountId: document.querySelector("#xingtuAccountId"),
  autoUploadEnabled: document.querySelector("#autoUploadEnabled"),
  uploadIntervalMinutes: document.querySelector("#uploadIntervalMinutes"),
  saveButton: document.querySelector("#saveButton"),
  uploadButton: document.querySelector("#uploadButton"),
  lastAction: document.querySelector("#lastAction"),
  lastUpdatedAt: document.querySelector("#lastUpdatedAt"),
  lastResponse: document.querySelector("#lastResponse"),
};

await boot();

async function boot() {
  const config = await getConfig();
  await renderConfig(config);
  await renderLastStatus();

  elements.saveButton.addEventListener("click", () => runAction("保存配置", saveCurrentConfig));
  elements.uploadButton.addEventListener("click", () => runAction("上传登录态", uploadSession));
}

async function renderConfig(config) {
  elements.distributionId.value = config.distributionId ?? DEFAULT_CONFIG.distributionId;
  elements.recipientName.value = config.recipientName ?? DEFAULT_CONFIG.recipientName;
  elements.serverBaseUrl.value = config.serverBaseUrl ?? DEFAULT_CONFIG.serverBaseUrl;
  elements.xingtuAccountId.value = config.xingtuAccountId ?? DEFAULT_CONFIG.xingtuAccountId;
  elements.autoUploadEnabled.checked = Boolean(config.autoUploadEnabled);
  elements.uploadIntervalMinutes.value =
    config.uploadIntervalMinutes ?? DEFAULT_CONFIG.uploadIntervalMinutes;
  elements.statusText.textContent = "配置已加载";
}

async function saveCurrentConfig() {
  const config = {
    autoUploadEnabled: elements.autoUploadEnabled.checked,
    uploadIntervalMinutes: Math.max(5, Number(elements.uploadIntervalMinutes.value) || 30),
  };

  await saveConfig(config);
  await chrome.runtime.sendMessage({ type: "RESET_AUTO_UPLOAD_ALARM" });
  elements.statusText.textContent = "配置已保存";
  return config;
}

async function uploadSession() {
  await saveCurrentConfig();
  const [tab] = await chrome.tabs.query({
    active: true,
    currentWindow: true,
  });
  return chrome.runtime.sendMessage({
    type: "UPLOAD_XINGTU_SESSION",
    tabId: tab?.id,
  });
}

async function runAction(label, action) {
  setBusy(true);
  elements.statusText.textContent = `${label}中`;

  try {
    const result = await action();
    if (result?.ok === false) {
      throw new Error(result.message || `${label}失败`);
    }

    elements.statusText.textContent = `${label}成功`;
    elements.lastAction.textContent = label;
    elements.lastUpdatedAt.textContent = formatDateTime(new Date().toISOString());
    elements.lastResponse.textContent = JSON.stringify(result?.data ?? result, null, 2);
    await renderLastStatus();
  } catch (error) {
    elements.statusText.textContent = `${label}失败：${formatError(error)}`;
  } finally {
    setBusy(false);
  }
}

async function renderLastStatus() {
  const status = await getLastStatus();

  if (!status) {
    elements.lastAction.textContent = "-";
    elements.lastUpdatedAt.textContent = "-";
    elements.lastResponse.textContent = "{}";
    return;
  }

  elements.lastAction.textContent = status.action ?? "-";
  elements.lastUpdatedAt.textContent = formatDateTime(status.updatedAt);
  elements.lastResponse.textContent = JSON.stringify(status.response ?? status, null, 2);
}

function setBusy(isBusy) {
  for (const button of [elements.saveButton, elements.uploadButton]) {
    button.disabled = isBusy;
  }
}

function formatDateTime(value) {
  if (!value) {
    return "-";
  }

  return new Date(value).toLocaleString("zh-CN", {
    hour12: false,
  });
}
