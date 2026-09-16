import { describe, expect, test } from "bun:test";
import { apiPath } from "./urls";

describe("apiPath", () => {
  test("keeps root API paths unchanged", () => {
    expect(apiPath("/api/data-sync/projects")).toBe(
      "/api/data-sync/projects",
    );
  });

  test("adds a leading slash", () => {
    expect(apiPath("health")).toBe("/health");
  });
});
