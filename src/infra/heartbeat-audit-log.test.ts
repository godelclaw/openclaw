import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  appendHeartbeatAuditLog,
  readLastHeartbeatAudit,
  resolveHeartbeatAuditPaths,
} from "./heartbeat-audit-log.js";

describe("heartbeat audit log", () => {
  it("appends JSONL and updates summary", async () => {
    const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "heartbeat-audit-"));
    const entry = {
      ts: 1_741_686_400_000,
      status: "ok-token" as const,
      reason: "interval",
      agentId: "main",
      sessionKey: "agent:main:main",
    };

    await appendHeartbeatAuditLog(entry, stateDir);

    const { logPath } = resolveHeartbeatAuditPaths(stateDir);
    const logRaw = await fs.readFile(logPath, "utf-8");
    expect(logRaw).toContain("\"status\":\"ok-token\"");

    const summary = await readLastHeartbeatAudit(stateDir);
    expect(summary?.status).toBe("ok-token");
    expect(summary?.sessionKey).toBe("agent:main:main");
  });
});
