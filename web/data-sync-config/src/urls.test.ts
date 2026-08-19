import { describe, expect, test } from "bun:test";
import { apiPath } from "./urls";

describe("apiPath", () => {
  test("keeps root API paths unchanged", () => {
    expect(apiPath("/api/v1/queries/projects")).toBe(
      "/api/v1/queries/projects",
    );
  });

  test("adds a leading slash", () => {
    expect(apiPath("health")).toBe("/health");
  });
});
