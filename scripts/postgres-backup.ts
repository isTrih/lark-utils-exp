import { dirname, resolve } from "node:path";
import { mkdir } from "node:fs/promises";

const [command, fileArgument, ...flags] = Bun.argv.slice(2);
const databaseUrl = process.env.DATABASE_URL?.trim();
if (!databaseUrl) throw new Error("缺少 DATABASE_URL");
const parsedDatabaseUrl = new URL(databaseUrl);
if (!['postgres:', 'postgresql:'].includes(parsedDatabaseUrl.protocol)) {
  throw new Error("DATABASE_URL 必须使用 postgres:// 或 postgresql://");
}
const databaseName = decodeURIComponent(parsedDatabaseUrl.pathname.replace(/^\//, ""));
if (!parsedDatabaseUrl.hostname || !databaseName || !parsedDatabaseUrl.username) {
  throw new Error("DATABASE_URL 缺少 host、username 或 database");
}
const connectionArguments = [
  "--host", parsedDatabaseUrl.hostname,
  "--port", parsedDatabaseUrl.port || "5432",
  "--username", decodeURIComponent(parsedDatabaseUrl.username),
  "--dbname", databaseName,
];
const pgEnvironment = {
  ...Bun.env,
  PGPASSWORD: decodeURIComponent(parsedDatabaseUrl.password),
  ...(parsedDatabaseUrl.searchParams.get("sslmode")
    ? { PGSSLMODE: parsedDatabaseUrl.searchParams.get("sslmode")! }
    : {}),
};
if (!fileArgument) {
  throw new Error(
    "用法: bun scripts/postgres-backup.ts backup <file.dump> | restore <file.dump> --confirm-restore",
  );
}
const file = resolve(fileArgument);

if (command === "backup") {
  await mkdir(dirname(file), { recursive: true });
  await run([
    "pg_dump",
    ...connectionArguments,
    "--format=custom",
    "--no-owner",
    "--no-privileges",
    "--file",
    file,
  ]);
  console.log(`备份完成: ${file}`);
} else if (command === "restore") {
  if (!flags.includes("--confirm-restore")) {
    throw new Error("恢复会覆盖目标数据库对象，必须显式传入 --confirm-restore");
  }
  if (!(await Bun.file(file).exists())) throw new Error(`备份文件不存在: ${file}`);
  await run([
    "pg_restore",
    ...connectionArguments,
    "--clean",
    "--if-exists",
    "--no-owner",
    "--no-privileges",
    file,
  ]);
  console.log(`恢复完成: ${file}`);
} else {
  throw new Error(`未知命令: ${command || "(empty)"}`);
}

async function run(argv: string[]) {
  const process = Bun.spawn(argv, {
    env: pgEnvironment,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await process.exited;
  if (exitCode !== 0) throw new Error(`${argv[0]} 执行失败，exit=${exitCode}`);
}
