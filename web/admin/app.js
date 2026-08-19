const $ = (selector) => document.querySelector(selector);
const token = () => $("#token").value.trim();

async function api(path, options = {}) {
  const response = await fetch(`/api/v1/admin/${path}`, {
    ...options,
    headers: { Authorization: `Bearer ${token()}`, "Content-Type": "application/json", ...(options.headers || {}) },
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.message || `${response.status} ${response.statusText}`);
  return body;
}

function table(container, columns, rows, actions) {
  container.replaceChildren();
  const element = document.createElement("table");
  const head = element.createTHead().insertRow();
  for (const [key, title] of columns) { const th = document.createElement("th"); th.textContent = title; head.append(th); }
  if (actions) { const th = document.createElement("th"); th.textContent = "操作"; head.append(th); }
  const body = element.createTBody();
  for (const row of rows) {
    const tr = body.insertRow();
    for (const [key] of columns) { const td = tr.insertCell(); td.textContent = row[key] ?? "-"; if (key.includes("error")) td.className = "error"; }
    if (actions) tr.append(actions(row));
  }
  container.append(element);
}

function show(message) { const node = $("#message"); node.textContent = message; node.style.display = "block"; setTimeout(() => node.style.display = "none", 3500); }

async function refresh() {
  if (!token()) return show("请先输入 Bearer Token");
  try {
    const [status, projects, runs, failures, quarantine] = await Promise.all([
      api("status"), api("periods/statuses"), api("workflow-runs?limit=30"), api("failed-sources?limit=30"), api("quarantine?limit=30"),
    ]);
    $("#status").replaceChildren(...Object.entries(status).map(([key, value]) => { const card = document.createElement("div"); card.className = "card"; const label = document.createElement("span"); label.textContent = key; const strong = document.createElement("strong"); strong.textContent = value ?? "-"; card.append(label, strong); return card; }));
    table($("#projects"), [["activity_period_id","ID"],["project","项目"],["period","期次"],["account_status","账号"],["latest_success_at","最近成功"],["latest_failure_at","最近失败"],["latest_data_at","数据新鲜度"],["pending_sources","待导入"],["failed_sources","失败"],["quarantine_rows","隔离"]], projects);
    table($("#runs"), [["workflow_run_id","ID"],["workflow_kind","类型"],["status","状态"],["trigger_source","触发"],["started_at","开始"],["finished_at","结束"],["error_message","错误"]], runs);
    table($("#failures"), [["feishu_source_id","来源ID"],["project","项目"],["period","期次"],["content_type","类型"],["attempt_count","次数"],["next_retry_at","下次重试"],["dead_letter_at","Dead letter"],["error_message","错误"]], failures, (row) => { const td = document.createElement("td"); for (const action of ["retry","ignore"]) { const button = document.createElement("button"); button.textContent = action === "retry" ? "重试" : "忽略"; button.addEventListener("click", async () => { await api(`failed-sources/${row.feishu_source_id}/${action}`, {method:"POST"}); await refresh(); }); td.append(button, " "); } return td; });
    table($("#quarantine"), [["quarantine_id","ID"],["content_type","类型"],["unique_key","业务键"],["reason_message","原因"],["created_at","时间"]], quarantine);
  } catch (error) { show(error.message); }
}

document.querySelector('[data-action="refresh"]').addEventListener("click", refresh);
