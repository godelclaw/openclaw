import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { appendHeartbeatTurnLog } from "../infra/recent-turn-window.js";
import { resolveHeartbeatContextObservedInput } from "./heartbeat-context.js";

describe("heartbeat context", () => {
  const tempRoots: string[] = [];

  afterEach(async () => {
    while (tempRoots.length > 0) {
      const root = tempRoots.pop();
      if (root) {
        await fs.rm(root, { recursive: true, force: true });
      }
    }
  });

  async function makeRoot(): Promise<string> {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), "heartbeat-context-"));
    tempRoots.push(root);
    return root;
  }

  it("observes reserved main telegram turns and content-bearing heartbeats separately from noop trace", async () => {
    const root = await makeRoot();
    const historyRoot = path.join(root, "memory", "history");
    const auditDir = path.join(historyRoot, "audit");
    const stateDir = path.join(root, "state");
    await fs.mkdir(auditDir, { recursive: true });
    await fs.writeFile(
      path.join(auditDir, "2026-03-11.jsonl"),
      [
        JSON.stringify({
          ts: 1_700_000_001,
          channel: "telegram",
          kind: "driver_turn",
          session_key: "agent:main:main",
          user_text: "zar asks",
          assistant_text: "oruzi answers",
        }),
        JSON.stringify({
          ts: 1_700_000_002,
          channel: "webchat",
          kind: "driver_turn",
          session_key: "agent:main:web",
          user_text: "web asks",
          assistant_text: "web answers",
        }),
        JSON.stringify({
          ts: 1_700_000_003,
          channel: "webchat",
          kind: "driver_turn",
          session_key: "agent:main:web",
          user_text: "web asks again",
          assistant_text: "web answers again",
        }),
      ].join("\n") + "\n",
      "utf-8",
    );

    await appendHeartbeatTurnLog(
      {
        ts: 1_700_000_002_500,
        sessionKey: "agent:main:main",
        status: "sent",
        userText: "Heartbeat",
        assistantText: "Checked Moltbook and left a note.",
      },
      stateDir,
    );
    await appendHeartbeatTurnLog(
      {
        ts: 1_700_000_004_000,
        sessionKey: "agent:main:main",
        status: "ok-token",
        userText: "Heartbeat",
        assistantText: "HEARTBEAT_OK",
      },
      stateDir,
    );

    const observed = await resolveHeartbeatContextObservedInput({
      historyRoot,
      statePath: stateDir,
      mainSessionKey: "agent:main:main",
      recentTurnLimit: 5,
      heartbeatTraceLimit: 32,
    });

    expect(observed).toEqual({
      recent_turn_count: 4,
      recent_turn_limit: 5,
      reserved_candidate_present: true,
      reserved_included: true,
      content_heartbeat_candidate_present: true,
      content_heartbeat_included: true,
      noop_heartbeat_in_recent_turns: false,
      heartbeat_trace_count: 2,
      heartbeat_trace_limit: 32,
      heartbeat_trace_oldest_first: true,
      heartbeat_trace_only_heartbeat_entries: true,
    });
  });

  it("respects a disabled trace block", async () => {
    const root = await makeRoot();
    const historyRoot = path.join(root, "memory", "history");
    await fs.mkdir(path.join(historyRoot, "audit"), { recursive: true });

    const observed = await resolveHeartbeatContextObservedInput({
      historyRoot,
      statePath: path.join(root, "state"),
      mainSessionKey: "agent:main:main",
      recentTurnLimit: 5,
      heartbeatTraceLimit: 0,
    });

    expect(observed.heartbeat_trace_count).toBe(0);
    expect(observed.heartbeat_trace_limit).toBe(0);
    expect(observed.heartbeat_trace_oldest_first).toBe(true);
  });
});
