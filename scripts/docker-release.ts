import { resolve } from "node:path";

const projectRoot = resolve(import.meta.dir, "..");
const args = new Set(Bun.argv.slice(2));
const supportedArgs = new Set(["--allow-dirty", "--dry-run"]);
const unknownArgs = [...args].filter((arg) => !supportedArgs.has(arg));

if (unknownArgs.length > 0) {
  throw new Error(`未知参数: ${unknownArgs.join(", ")}`);
}

const allowDirty = args.has("--allow-dirty");
const dryRun = args.has("--dry-run");
const image = process.env.DOCKER_IMAGE?.trim() || "trihlp/lark-utils-exp";
const platform = process.env.DOCKER_PLATFORM?.trim() || "linux/amd64";

const metadata = JSON.parse(
  commandOutput(["cargo", "metadata", "--no-deps", "--format-version", "1"]),
) as {
  packages: Array<{ name: string; version: string; manifest_path: string }>;
};
const packageInfo = metadata.packages.find(
  (item) => item.name === "lark-exp" && item.manifest_path === `${projectRoot}/Cargo.toml`,
);

if (!packageInfo) {
  throw new Error("Cargo metadata 中没有找到 lark-exp 根 package");
}

const baseVersion = packageInfo.version;
if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/.test(baseVersion)) {
  throw new Error(`Cargo.toml version 不是有效的发布 SemVer: ${baseVersion}`);
}

if (!allowDirty) {
  const trackedChanges = commandOutput([
    "git",
    "status",
    "--porcelain",
    "--untracked-files=no",
  ]);
  if (trackedChanges) {
    throw new Error("存在未提交的已跟踪文件；请先提交，或仅调试时传入 --allow-dirty");
  }
}

const gitCommit = commandOutput(["git", "rev-parse", "--short=12", "HEAD"]);
const builtAt = new Date().toISOString();
const buildStamp = builtAt.replace(/[-:]/g, "").replace(/\.\d{3}Z$/, "Z");
const buildMetadata = `build.${buildStamp}.git.${gitCommit}`;
const fullVersion = `${baseVersion}+${buildMetadata}`;
const platformSuffix = platform.split("/").at(-1)?.replace(/[^0-9A-Za-z_.-]/g, "-") || "image";
const immutableTag = `${baseVersion}-build.${buildStamp}.git.${gitCommit}-${platformSuffix}`;
const tags = ["latest", baseVersion, immutableTag].map((tag) => `${image}:${tag}`);
const command = [
  "docker",
  "buildx",
  "build",
  "--platform",
  platform,
  "--push",
  "--build-arg",
  `APP_VERSION=${fullVersion}`,
  "--build-arg",
  `BUILD_METADATA=${buildMetadata}`,
  "--build-arg",
  `GIT_COMMIT=${gitCommit}`,
  "--build-arg",
  `BUILD_DATE=${builtAt}`,
  ...tags.flatMap((tag) => ["-t", tag]),
  ".",
];

console.log(`基础版本: ${baseVersion}`);
console.log(`完整版本: ${fullVersion}`);
console.log(`Git 提交: ${gitCommit}`);
console.log(`构建时间: ${builtAt}`);
console.log(`镜像标签:\n${tags.map((tag) => `  ${tag}`).join("\n")}`);

if (dryRun) {
  console.log(`Dry run 命令:\n${command.map(shellQuote).join(" ")}`);
  process.exit(0);
}

const result = Bun.spawnSync(command, {
  cwd: projectRoot,
  stdin: "inherit",
  stdout: "inherit",
  stderr: "inherit",
});

if (result.exitCode !== 0) {
  process.exit(result.exitCode);
}

function commandOutput(command: string[]): string {
  const result = Bun.spawnSync(command, {
    cwd: projectRoot,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) {
    const stderr = new TextDecoder().decode(result.stderr).trim();
    throw new Error(`${command.join(" ")} 执行失败: ${stderr}`);
  }
  return new TextDecoder().decode(result.stdout).trim();
}

function shellQuote(value: string): string {
  return /^[0-9A-Za-z_./:=+-]+$/.test(value)
    ? value
    : `'${value.replaceAll("'", `'\\''`)}'`;
}
