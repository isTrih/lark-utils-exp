import { describe, expect, test } from "bun:test";
import {
  buildPackageManifest,
  createInstanceConfigModule,
  validatePackageConfig,
} from "./package-xingtu-extension";

function validConfig() {
  return {
    version: 1,
    serverBaseUrl: "https://api.example.com",
    sessionUploadToken: "x".repeat(40),
    defaults: { autoUploadEnabled: true, uploadIntervalMinutes: 30 },
    packages: [
      {
        distributionId: "operator-a",
        recipientName: "运营 A",
        xingtuAccountId: "account-a",
        enabled: true,
      },
    ],
  };
}

describe("xingtu extension package config", () => {
  test("validates and generates one isolated instance config", () => {
    const config = validatePackageConfig(validConfig());
    const entry = config.packages[0];
    const moduleText = createInstanceConfigModule(config, entry);
    expect(moduleText).toContain('"distributionId": "operator-a"');
    expect(moduleText).toContain('"xingtuAccountId": "account-a"');
    expect(moduleText).toContain('"sessionUploadToken": "xxxxxxxx');
    expect(moduleText).not.toContain("packages");

    const manifest = buildPackageManifest(
      { version: "0.3.0" },
      config,
      entry,
    );
    expect(manifest.host_permissions).toContain("https://api.example.com/*");
    expect(String(manifest.name)).toContain("运营 A");
  });

  test("rejects placeholder tokens and duplicate distribution ids", () => {
    expect(() =>
      validatePackageConfig({
        ...validConfig(),
        sessionUploadToken: "replace-with-a-real-token-at-least-32-chars",
      }),
    ).toThrow("真实随机 Token");
    const duplicate = validConfig();
    duplicate.packages.push({ ...duplicate.packages[0] });
    expect(() => validatePackageConfig(duplicate)).toThrow("distributionId 重复");
  });
});
