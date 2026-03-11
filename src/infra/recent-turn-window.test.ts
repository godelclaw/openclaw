import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  appendHeartbeatTurnLog,
  loadRecentHeartbeatTrace,
  loadRecentTurns,
  renderHeartbeatTraceBlock,
  renderRecentTurnsBlock,
  selectRecentTurns,
  type RecentTurnWindowEntry,
} from "./recent-turn-window.js";

async function withTempDir<T>(prefix: string, run: (root: string) => Promise<T>): Promise<T> {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), prefix));
  try {
    return await run(root);
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
}

describe("recent turn window", () => {
  it("keeps one recent main telegram turn when selecting the last k entries", () => {
    const entries: RecentTurnWindowEntry[] = [
      {
        ts: 1,
        source: "telegram",
        sessionKey: "agent:main:main",
        userText: "earlier zar",
        assistantText: "earlier reply",
        reserveMainTelegram: true,
        noopHeartbeat: false,
      },
      {
        ts: 2,
        source: "heartbeat",
        sessionKey: "agent:main:main",
        userText: "Heartbeat",
        assistantText: "HEARTBEAT_OK",
        reserveMainTelegram: false,
        noopHeartbeat: true,
      },
      {
        ts: 3,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web one",
        assistantText: "web reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 4,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web two",
        assistantText: "web reply two",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 5,
        source: "heartbeat",
        sessionKey: "agent:main:main",
        userText: "Heartbeat",
        assistantText: "HEARTBEAT_OK",
        reserveMainTelegram: false,
        noopHeartbeat: true,
      },
      {
        ts: 6,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "latest web",
        assistantText: "latest web reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 7,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "newest web",
        assistantText: "newest web reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
    ];

    const selected = selectRecentTurns({ entries, limit: 5 });
    expect(selected).toHaveLength(5);
    expect(
      selected.some(
        (entry) => entry.source === "telegram" && entry.sessionKey === "agent:main:main",
      ),
    ).toBe(true);
  });

  it("does not let a HEARTBEAT_OK entry push out a content-bearing heartbeat", () => {
    const entries: RecentTurnWindowEntry[] = [
      {
        ts: 1,
        source: "telegram",
        sessionKey: "agent:main:main",
        userText: "zar asks",
        assistantText: "main reply",
        reserveMainTelegram: true,
        noopHeartbeat: false,
      },
      {
        ts: 2,
        source: "heartbeat",
        sessionKey: "agent:main:main",
        userText: "Heartbeat",
        assistantText: "Checked Moltbook and left a note.",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 3,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web one",
        assistantText: "web one reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 4,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web two",
        assistantText: "web two reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 5,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web three",
        assistantText: "web three reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 6,
        source: "heartbeat",
        sessionKey: "agent:main:main",
        userText: "Heartbeat",
        assistantText: "HEARTBEAT_OK",
        reserveMainTelegram: false,
        noopHeartbeat: true,
      },
      {
        ts: 7,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web four",
        assistantText: "web four reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
      {
        ts: 8,
        source: "webchat",
        sessionKey: "agent:main:web",
        userText: "web five",
        assistantText: "web five reply",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
    ];

    const selected = selectRecentTurns({ entries, limit: 5 });
    expect(selected).toHaveLength(5);
    expect(
      selected.some(
        (entry) => entry.source === "heartbeat" && entry.assistantText === "Checked Moltbook and left a note.",
      ),
    ).toBe(true);
    expect(selected.some((entry) => entry.noopHeartbeat)).toBe(false);
  });

  it("loads driver audit turns and content-bearing heartbeat turns together", async () => {
    await withTempDir("openclaw-recent-turns-", async (root) => {
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
            assistant_text: "web answer",
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
          ts: 1_700_000_003_000,
          sessionKey: "agent:main:main",
          status: "ok-token",
          userText: "Heartbeat",
          assistantText: "HEARTBEAT_OK",
        },
        stateDir,
      );

      const entries = await loadRecentTurns({
        historyRoot,
        statePath: stateDir,
        mainSessionKey: "agent:main:main",
        limit: 5,
      });

      expect(entries.map((entry) => entry.source)).toContain("telegram");
      expect(entries.map((entry) => entry.source)).toContain("heartbeat");
      const rendered = renderRecentTurnsBlock(entries, 5);
      expect(rendered).toContain("Recent completed turns");
      expect(rendered).toContain("zar asks");
      expect(rendered).toContain("Checked Moltbook and left a note.");
      expect(rendered).not.toContain("HEARTBEAT_OK");

      const heartbeatTrace = await loadRecentHeartbeatTrace({ statePath: stateDir });
      const renderedTrace = renderHeartbeatTraceBlock(heartbeatTrace);
      expect(renderedTrace).toContain("Heartbeat trace");
      expect(renderedTrace).toContain("Checked Moltbook and left a note.");
      expect(renderedTrace).toContain("HEARTBEAT_OK");
    });
  });
});
