import { readdir, stat, unlink } from "node:fs/promises";
import { join } from "node:path";

const logDir = process.env.LOG_DIR?.trim() || "/app/logs";
const retentionDays = Number.parseInt(process.env.LOG_RETENTION_DAYS || "30", 10);
const dryRun = Bun.argv.includes("--dry-run");
if (!Number.isInteger(retentionDays) || retentionDays < 1) {
  throw new Error("LOG_RETENTION_DAYS 必须是正整数");
}
const cutoff = Date.now() - retentionDays * 24 * 60 * 60 * 1000;
let removed = 0;
for (const name of await readdir(logDir)) {
  if (!name.startsWith("lark-utils-exp.jsonl.")) continue;
  const path = join(logDir, name);
  const metadata = await stat(path);
  if (!metadata.isFile() || metadata.mtimeMs >= cutoff) continue;
  if (dryRun) console.log(`[dry-run] ${path}`);
  else await unlink(path);
  removed += 1;
}
console.log(`${dryRun ? "将清理" : "已清理"} ${removed} 个超过 ${retentionDays} 天的日志文件`);
