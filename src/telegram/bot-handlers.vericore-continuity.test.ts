import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { OpenClawConfig } from "../config/config.js";
import { appendHeartbeatTurnLog } from "../infra/recent-turn-window.js";
import { buildVeriCoreContinuityContextForSession } from "./bot-handlers.js";

const tempRoots: string[] = [];

afterEach(async () => {
  await Promise.all(
    tempRoots.splice(0).map(async (root) => {
      await fs.rm(root, { recursive: true, force: true });
    }),
  );
});

describe("buildVeriCoreContinuityContextForSession", () => {
  it("includes recent completed turns and heartbeat trace for Telegram sessions", async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "vericore-continuity-"));
    tempRoots.push(root);
    const workspaceDir = path.join(root, "workspace");
    const historyRoot = path.join(workspaceDir, "memory", "history");
    const auditDir = path.join(historyRoot, "audit");
    const statePath = path.join(root, "state");
    await fs.mkdir(auditDir, { recursive: true });
    await fs.mkdir(path.join(statePath, "logs"), { recursive: true });

    const auditLines = [
      {
        ts: 1773310400,
        channel: "telegram_dm",
        kind: "driver_turn",
        session_key: "agent:main:main",
        user_text: "Hey, how's moltbook? :)",
        assistant_text: "Moltbook is alive and well!",
      },
      {
        ts: 1773310500,
        channel: "telegram_dm",
        kind: "driver_turn",
        session_key: "agent:main:main",
        user_text: "What were we doing?",
        assistant_text: "We were looking at Moltbook threads and architecture notes.\n\n⋄⟨Cn:.5 C:.7 Ct:.8 I:.2 J:.6 A:.4 S:.6⟩",
      },
    ];
    await fs.writeFile(
      path.join(auditDir, "2026-03-12.jsonl"),
      auditLines.map((line) => JSON.stringify(line)).join("\n") + "\n",
      "utf8",
    );
    await appendHeartbeatTurnLog(
      {
        ts: 1773310555000,
        sessionKey: "agent:main:main",
        status: "ok-token",
        userText: "Heartbeat. Read HEARTBEAT.md and follow it calmly.",
        assistantText: "HEARTBEAT_OK",
      },
      statePath,
    );

    const cfg: OpenClawConfig = {
      agents: {
        defaults: {
          workspace: workspaceDir,
          heartbeat: { traceLimit: 32 },
        },
        list: [{ id: "main", default: true, workspace: workspaceDir }],
      },
    };

    const body = await buildVeriCoreContinuityContextForSession({
      cfg,
      sessionKey: "agent:main:main",
      statePath,
    });

    expect(body).toContain("(energy 0)");
    expect(body).toContain("Recent completed turns");
    expect(body).toContain("Hey, how's moltbook? :)");
    expect(body).toContain("Moltbook is alive and well!");
    expect(body).toContain("Heartbeat trace");
    expect(body).toContain("HEARTBEAT_OK");
    expect(body).toContain("Affect trace aggregate:");
    expect(body).toContain("⋄⟨");
    expect(body).toContain("Cn:.5");
  });
});
