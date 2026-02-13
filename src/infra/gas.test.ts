import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { formatGasAlertMessage, refreshAgentGasStatus, resolveGasCreditsTag } from "./gas.js";

describe("refreshAgentGasStatus", () => {
  const originalEnv = { ...process.env };

  afterEach(async () => {
    process.env = { ...originalEnv };
    vi.unstubAllGlobals();
  });

  it("writes GAS.md and emits threshold alerts once", async () => {
    const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-gas-state-"));
    const workspaceDir = path.join(stateDir, "workspace-main");

    process.env.OPENCLAW_STATE_DIR = stateDir;
    process.env.OPENROUTER_API_KEY = "test-key";

    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            data: {
              usage_daily: 0.9,
              usage_weekly: 2.4,
              usage_monthly: 9.5,
              usage: 11.2,
              limit: null,
              limit_remaining: null,
            },
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            data: {
              usage_daily: 0.95,
              usage_weekly: 2.45,
              usage_monthly: 9.55,
              usage: 11.25,
              limit: null,
              limit_remaining: null,
            },
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        ),
      );

    vi.stubGlobal("fetch", fetchMock);

    const cfg = {
      agents: {
        defaults: {
          workspace: workspaceDir,
          gas: {
            enabled: true,
            dailyUsd: 1,
            weeklyUsd: 12,
            monthlyUsd: 48,
            thresholds: [50, 75, 90, 100],
            alerts: {
              enabled: true,
              periods: ["daily", "weekly", "monthly"],
            },
          },
        },
      },
    };

    const first = await refreshAgentGasStatus({ cfg, agentId: "main" });
    expect(first).not.toBeNull();
    expect(first?.alerts.map((entry) => `${entry.period}:${entry.threshold}`)).toEqual([
      "daily:90",
    ]);

    const gasPath = path.join(workspaceDir, "GAS.md");
    const gasMd = await fs.readFile(gasPath, "utf-8");
    expect(gasMd).toContain("# GAS.md");
    expect(gasMd).toContain("Today: $0.9000 | Week: $2.4000 | Month: $9.5000 | Total: $11.2000");

    const tag = await resolveGasCreditsTag({ cfg, agentId: "main" });
    expect(tag).toBe("[$0.90 today]");

    const second = await refreshAgentGasStatus({ cfg, agentId: "main" });
    expect(second).not.toBeNull();
    expect(second?.alerts).toHaveLength(0);

    const text = formatGasAlertMessage(first?.alerts);
    expect(text).toBe("Daily gas reached: 90%.");
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it("returns null when gas is disabled or not configured", async () => {
    const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-gas-state-"));
    process.env.OPENCLAW_STATE_DIR = stateDir;
    process.env.OPENROUTER_API_KEY = "test-key";

    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const cfg = {
      agents: {
        defaults: {
          workspace: path.join(stateDir, "workspace-main"),
        },
      },
    };

    const result = await refreshAgentGasStatus({ cfg, agentId: "main" });
    expect(result).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
