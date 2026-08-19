// 安全的未配置占位文件。正式分发包由 scripts/package-xingtu-extension.ts 在临时目录中
// 覆盖本文件；不要在仓库里填写真实 Token 或账号 ID。
export const INSTANCE_CONFIG = Object.freeze({
  distributionId: "unconfigured",
  recipientName: "未配置",
  serverBaseUrl: "https://api.example.com",
  xingtuAccountId: "",
  sessionUploadToken: "",
  autoUploadEnabled: true,
  uploadIntervalMinutes: 30,
});
