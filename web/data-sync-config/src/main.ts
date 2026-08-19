import { bitable } from "@lark-base-open/connector-api";
import { apiPath } from "./urls";

interface Project {
  activity_period_id: number;
  project_display_name: string;
  period: string;
}

function requiredElement<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) {
    throw new Error(`配置页面缺少元素：${selector}`);
  }
  return element;
}

const form = requiredElement<HTMLFormElement>("#config-form");
const select = requiredElement<HTMLSelectElement>("#project-select");
const saveButton = requiredElement<HTMLButtonElement>("#save-button");
const status = requiredElement<HTMLParagraphElement>("#status");

function setStatus(message: string, error = false): void {
  status.textContent = message;
  status.dataset.error = String(error);
}

function savedProjectId(config: Record<string, unknown>): number | undefined {
  const value = config.activity_period_id ?? config.activityPeriodId;
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : undefined;
}

async function readSavedConfig(): Promise<Record<string, unknown>> {
  const timeout = new Promise<Record<string, unknown>>((resolve) => {
    window.setTimeout(() => resolve({}), 3_000);
  });
  return Promise.race([bitable.getConfig(), timeout]);
}

async function loadConfig(): Promise<void> {
  try {
    const response = await fetch(apiPath("/api/v1/queries/projects"), {
      headers: { Accept: "application/json" },
    });
    if (!response.ok) {
      throw new Error(`读取项目失败（HTTP ${response.status}）`);
    }
    const projects = (await response.json()) as Project[];
    if (projects.length === 0) {
      select.innerHTML = '<option value="">暂无可同步项目</option>';
      setStatus("当前没有启用的项目", true);
      return;
    }

    select.innerHTML = "";
    for (const project of projects) {
      const option = document.createElement("option");
      option.value = String(project.activity_period_id);
      option.textContent = `${project.project_display_name} - ${project.period}`;
      select.append(option);
    }
    select.disabled = false;
    saveButton.disabled = false;
    setStatus(`${projects.length} 个可同步项目`);

    try {
      const configuredProjectId = savedProjectId(await readSavedConfig());
      if (
        configuredProjectId &&
        projects.some(
          (project) => project.activity_period_id === configuredProjectId,
        )
      ) {
        select.value = String(configuredProjectId);
      }
    } catch {
      // 首次配置或直接在浏览器打开时没有宿主配置，保持默认项目即可。
    }
  } catch (error) {
    setStatus(error instanceof Error ? error.message : String(error), true);
  }
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const activityPeriodId = Number(select.value);
  if (!Number.isSafeInteger(activityPeriodId) || activityPeriodId <= 0) {
    setStatus("请选择同步项目", true);
    return;
  }

  select.disabled = true;
  saveButton.disabled = true;
  setStatus("正在保存...");
  try {
    await bitable.saveConfigAndGoNext({
      activity_period_id: activityPeriodId,
    });
  } catch (error) {
    setStatus(error instanceof Error ? error.message : String(error), true);
    select.disabled = false;
    saveButton.disabled = false;
  }
});

void loadConfig();
