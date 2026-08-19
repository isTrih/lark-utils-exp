const SESSION_KEY_CANDIDATES = [
  "XINGTU_SESSION_KEY",
  "session_key",
  "sessionKey",
  "__xingtu_session_key__",
];

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type !== "XINGTU_COLLECT_PAGE_STATE") {
    return false;
  }

  sendResponse({
    ok: true,
    data: collectPageState(),
  });
  return true;
});

function collectPageState() {
  return {
    userAgent: navigator.userAgent,
    sessionKey: findSessionKey(),
    pageUrl: location.href,
  };
}

function findSessionKey() {
  for (const key of SESSION_KEY_CANDIDATES) {
    const value = localStorage.getItem(key) || sessionStorage.getItem(key);
    if (isUsefulString(value)) {
      return value.trim();
    }
  }

  const storageValue = scanStorageLike(localStorage) || scanStorageLike(sessionStorage);
  if (storageValue) {
    return storageValue;
  }

  // 部分站点会把登录态散在全局变量里；这里做轻量扫描，不读取 DOM 文本。
  for (const key of Object.keys(window)) {
    if (!/session.?key/i.test(key)) {
      continue;
    }

    try {
      const value = window[key];
      if (typeof value === "string" && isUsefulString(value)) {
        return value.trim();
      }
    } catch (_error) {
      // 某些浏览器内置属性读取会抛错，跳过即可。
    }
  }

  return null;
}

function scanStorageLike(storage) {
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (!key || !/session.?key/i.test(key)) {
      continue;
    }

    const value = storage.getItem(key);
    if (isUsefulString(value)) {
      return value.trim();
    }
  }

  return null;
}

function isUsefulString(value) {
  return typeof value === "string" && value.trim().length > 0;
}
