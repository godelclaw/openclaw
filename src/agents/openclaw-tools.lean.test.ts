import { describe, expect, it } from "vitest";
import "./test-helpers/fast-core-tools.js";
import { createOpenClawTools } from "./openclaw-tools.js";

describe("openclaw tools", () => {
  it("includes the lean tool", () => {
    const names = new Set(createOpenClawTools().map((tool) => tool.name));
    expect(names.has("lean")).toBe(true);
  });
});
