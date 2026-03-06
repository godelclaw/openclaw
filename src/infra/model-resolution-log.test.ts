import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  appendModelResolutionLog,
  classifyFallbackReason,
  formatModelFallbackAlert,
  formatModelResolutionStatusLine,
  readLastModelResolution,
  type ModelResolutionEntry,
} from "./model-resolution-log.js";

const tempDirs: string[] = [];

async function makeTempDir(): Promise<string> {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "model-resolution-log-"));
  tempDirs.push(dir);
  return dir;
}

function sampleEntry(overrides: Partial<ModelResolutionEntry> = {}): ModelResolutionEntry {
  return {
    ts: "2026-03-06T20:00:00.000Z",
    runId: "run-1",
    kind: "primary",
    configuredPrimary: "anthropic/claude-opus-4-6",
    configuredFallbacks: ["anthropic/claude-sonnet-4-6"],
    resolvedModel: "claude-sonnet-4-6",
    resolvedProvider: "anthropic",
    resolvedProfile: "anthropic:manual",
    modelChanged: true,
    providerChanged: false,
    fallbackReason: "timeout",
    attempts: [{ provider: "anthropic", model: "claude-opus-4-6", error: "timeout" }],
    usage: { input: 10, output: 3 },
    ...overrides,
  };
}

afterEach(async () => {
  await Promise.all(tempDirs.splice(0).map((dir) => fs.rm(dir, { recursive: true, force: true })));
});

describe("model-resolution-log", () => {
  it("writes jsonl and summary files and can read the summary back", async () => {
    const dir = await makeTempDir();
    const entry = sampleEntry();

    await appendModelResolutionLog(dir, entry);

    const jsonl = await fs.readFile(path.join(dir, "model-resolution.jsonl"), "utf-8");
    expect(jsonl.trim()).toBe(JSON.stringify(entry));
    expect(readLastModelResolution(dir)).toEqual(entry);
  });

  it("classifies common fallback reasons", () => {
    expect(classifyFallbackReason([{ error: "timeout contacting provider" }])).toBe("timeout");
    expect(classifyFallbackReason([{ error: "API error (402): Insufficient credits" }])).toBe(
      "billing",
    );
    expect(classifyFallbackReason([{ reason: "policy gate" }])).toBe("policy");
    expect(classifyFallbackReason([{ error: "auth missing" }])).toBe("auth");
    expect(classifyFallbackReason([])).toBeUndefined();
  });

  it("formats status and alert text with resolved truth", () => {
    const line = formatModelResolutionStatusLine(sampleEntry());
    expect(line).toContain("🧭 Last used: anthropic/claude-sonnet-4-6");
    expect(line).toContain("profile anthropic:manual");
    expect(line).toContain("model fallback: timeout");

    const alert = formatModelFallbackAlert({
      configuredPrimary: "anthropic/claude-opus-4-6",
      resolvedProvider: "openrouter",
      resolvedModel: "x-ai/grok-4.1-fast",
      providerChanged: true,
      fallbackReason: "billing",
    });
    expect(alert).toBe(
      "⚠️ Provider fallback: anthropic/claude-opus-4-6 → openrouter/x-ai/grok-4.1-fast (billing)",
    );
  });
});
