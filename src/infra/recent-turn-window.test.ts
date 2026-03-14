import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  aggregateAffectTraces,
  appendHeartbeatTurnLog,
  computeEnergy,
  foldAffectGestalt,
  loadAffectAggregateBlock,
  loadAffectGestalt,
  loadRecentHeartbeatTrace,
  loadRecentTurns,
  parseAffectTrace,
  renderAffectAggregateBlock,
  renderHeartbeatTraceBlock,
  renderRecentTurnsBlock,
  resolveAffectGestaltPath,
  selectRecentTurns,
  type AffectTrace,
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
  it("keeps one recent main telegram_dm turn when selecting the last k entries", () => {
    const entries: RecentTurnWindowEntry[] = [
      {
        ts: 1,
        source: "telegram_dm",
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
        (entry) => entry.source === "telegram_dm" && entry.sessionKey === "agent:main:main",
      ),
    ).toBe(true);
  });

  it("computes energy from heartbeat no-ops, heartbeat actions, and conversation bursts", () => {
    const entries: RecentTurnWindowEntry[] = [
      {
        ts: 1,
        source: "heartbeat",
        sessionKey: "agent:main:main",
        userText: "Heartbeat",
        assistantText: "HEARTBEAT_OK",
        reserveMainTelegram: false,
        noopHeartbeat: true,
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
        source: "telegram_dm",
        sessionKey: "agent:main:main",
        userText: "zar says hi",
        assistantText: "oruzi replies",
        reserveMainTelegram: true,
        noopHeartbeat: false,
      },
      {
        ts: 3 + 10 * 60 * 1000,
        source: "telegram_dm",
        sessionKey: "agent:main:main",
        userText: "zar follows up",
        assistantText: "oruzi follows up",
        reserveMainTelegram: true,
        noopHeartbeat: false,
      },
      {
        ts: 4 + 40 * 60 * 1000,
        source: "telegram_dm",
        sessionKey: "agent:main:main",
        userText: "zar restarts conversation",
        assistantText: "oruzi responds again",
        reserveMainTelegram: true,
        noopHeartbeat: false,
      },
      {
        ts: 5 + 40 * 60 * 1000,
        source: "heartbeat",
        sessionKey: "agent:main:main",
        userText: "Heartbeat",
        assistantText: "Checked Moltbook and wrote back.",
        reserveMainTelegram: false,
        noopHeartbeat: false,
      },
    ];

    expect(computeEnergy({ entries })).toBe(-1);
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
            channel: "telegram_dm",
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

      expect(entries.map((entry) => entry.source)).toContain("telegram_dm");
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

  describe("affect trace", () => {
    it("parses a valid terminal affect trace line", () => {
      const text = "Some reply content here.\n\n⋄⟨Cn:.5 C:.7 Ct:.8 I:.2 J:.6 A:.4 S:.6⟩";
      const result = parseAffectTrace(text);
      expect(result).toBeDefined();
      expect(result!.Cn).toBe(0.5);
      expect(result!.C).toBe(0.7);
      expect(result!.Ct).toBe(0.8);
      expect(result!.I).toBe(0.2);
      expect(result!.J).toBe(0.6);
      expect(result!.A).toBe(0.4);
      expect(result!.S).toBe(0.6);
    });

    it("accepts various numeric formats", () => {
      const text = "⋄⟨Cn:0.5 C:+.7 Ct:-.3 I:0 J:1 A:-1 S:.6⟩";
      const result = parseAffectTrace(text);
      expect(result).toBeDefined();
      expect(result!.Cn).toBe(0.5);
      expect(result!.C).toBe(0.7);
      expect(result!.Ct).toBe(-0.3);
      expect(result!.I).toBe(0);
      expect(result!.J).toBe(1);
      expect(result!.A).toBe(-1);
      expect(result!.S).toBe(0.6);
    });

    it("rejects text without affect trace marker", () => {
      expect(parseAffectTrace("Just a normal reply.")).toBeUndefined();
    });

    it("rejects trace with fewer than 7 dimensions", () => {
      const text = "⋄⟨Cn:.5 C:.7 Ct:.8⟩";
      expect(parseAffectTrace(text)).toBeUndefined();
    });

    it("rejects trace with wrong key order", () => {
      const text = "⋄⟨C:.7 Cn:.5 Ct:.8 I:.2 J:.6 A:.4 S:.6⟩";
      expect(parseAffectTrace(text)).toBeUndefined();
    });

    it("rejects trace with out-of-range values", () => {
      const text = "⋄⟨Cn:1.5 C:.7 Ct:.8 I:.2 J:.6 A:.4 S:.6⟩";
      expect(parseAffectTrace(text)).toBeUndefined();
    });

    it("only parses the last non-empty line", () => {
      const text = "I mentioned ⋄⟨Cn:0 C:0 Ct:0 I:0 J:0 A:0 S:0⟩ as an example.\n\n⋄⟨Cn:.5 C:.7 Ct:.8 I:.2 J:.6 A:.4 S:.6⟩\n";
      const result = parseAffectTrace(text);
      expect(result).toBeDefined();
      expect(result!.Cn).toBe(0.5);
    });

    it("aggregates affect traces with exponential discounting", () => {
      const entries: RecentTurnWindowEntry[] = [
        {
          ts: 1, source: "telegram_dm", userText: "a", assistantText: "r",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: { Cn: 0.5, C: 0.7, Ct: 0.8, I: 0.2, J: 0.6, A: 0.4, S: 0.6 },
        },
        {
          ts: 2, source: "telegram_dm", userText: "b", assistantText: "r",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: { Cn: 0.3, C: 0.5, Ct: 0.4, I: 0.0, J: 0.8, A: 0.2, S: 0.4 },
        },
        {
          ts: 3, source: "heartbeat", userText: "Heartbeat", assistantText: "HEARTBEAT_OK",
          reserveMainTelegram: false, noopHeartbeat: true,
        },
      ];
      const agg = aggregateAffectTraces(entries);
      expect(agg).toBeDefined();
      expect(agg!.Cn).toBe(0.4);
      expect(agg!.C).toBe(0.6);
      expect(agg!.J).toBe(0.7);
    });

    it("returns undefined when no entries have affect traces", () => {
      const entries: RecentTurnWindowEntry[] = [
        {
          ts: 1, source: "heartbeat", userText: "Heartbeat", assistantText: "HEARTBEAT_OK",
          reserveMainTelegram: false, noopHeartbeat: true,
        },
      ];
      expect(aggregateAffectTraces(entries)).toBeUndefined();
    });

    it("renders affect aggregate block in compact format", () => {
      const entries: RecentTurnWindowEntry[] = [
        {
          ts: 1, source: "telegram_dm", userText: "a", assistantText: "r",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: { Cn: 0.5, C: 0.7, Ct: 0.8, I: 0, J: 0.6, A: 0.4, S: 0.6 },
        },
      ];
      const block = renderAffectAggregateBlock(entries);
      expect(block).toContain("Affect trace aggregate:");
      expect(block).toContain("⋄⟨");
      expect(block).toContain("Cn:.5");
      expect(block).toContain("I:0");
      expect(block).toContain("⟩");
    });

    it("returns empty string when no affect traces exist", () => {
      expect(renderAffectAggregateBlock([])).toBe("");
    });

    it("weights newest entries more heavily regardless of array order", () => {
      const old: AffectTrace = { Cn: 0, C: 0, Ct: 0, I: 0, J: 0, A: 0, S: 0 };
      const recent: AffectTrace = { Cn: 1, C: 1, Ct: 1, I: 1, J: 1, A: 1, S: 1 };
      const entries: RecentTurnWindowEntry[] = [
        // Array order: recent first, then old — but ts says old is earlier
        {
          ts: 200, source: "telegram_dm", userText: "b", assistantText: "r",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: recent,
        },
        {
          ts: 100, source: "heartbeat", userText: "Heartbeat", assistantText: "acted",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: old,
        },
      ];
      // gamma=0.69: state starts at old (ts=100) = 0, then
      // state = 0.69*0 + 0.31*1 = 0.31, rounded to 0.3
      // Newest (ts=200, all-ones) pulls state UP from old (all-zeros)
      const agg = aggregateAffectTraces(entries);
      expect(agg).toBeDefined();
      expect(agg!.Cn).toBe(0.3);
      expect(agg!.C).toBe(0.3);
    });

    it("with a single entry, returns that entry's values", () => {
      const entries: RecentTurnWindowEntry[] = [
        {
          ts: 1, source: "telegram_dm", userText: "a", assistantText: "r",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: { Cn: 0.8, C: 0.3, Ct: 0.5, I: 0.1, J: 0.9, A: 0.4, S: 0.7 },
        },
      ];
      const agg = aggregateAffectTraces(entries);
      expect(agg).toBeDefined();
      expect(agg!.Cn).toBe(0.8);
      expect(agg!.J).toBe(0.9);
    });

    it("newest entry dominates with many entries", () => {
      // 5 old entries of zeros, then 1 new entry of ones
      const entries: RecentTurnWindowEntry[] = [];
      for (let i = 0; i < 5; i++) {
        entries.push({
          ts: i + 1, source: "telegram_dm", userText: "a", assistantText: "r",
          reserveMainTelegram: false, noopHeartbeat: false,
          affect: { Cn: 0, C: 0, Ct: 0, I: 0, J: 0, A: 0, S: 0 },
        });
      }
      entries.push({
        ts: 100, source: "telegram_dm", userText: "a", assistantText: "r",
        reserveMainTelegram: false, noopHeartbeat: false,
        affect: { Cn: 1, C: 1, Ct: 1, I: 1, J: 1, A: 1, S: 1 },
      });
      const agg = aggregateAffectTraces(entries);
      expect(agg).toBeDefined();
      // After 5 zeros, state ≈ 0. Then 0.69*0 + 0.31*1 = 0.31
      expect(agg!.Cn).toBe(0.3);
    });

    it("steps the gestalt with one clean gamma recurrence", () => {
      const previous: AffectTrace = { Cn: 0.5, C: 0.7, Ct: 0.8, I: 0.2, J: 0.6, A: 0.4, S: 0.6 };
      const next: AffectTrace = { Cn: 0.3, C: 0.5, Ct: 0.4, I: 0, J: 0.8, A: 0.2, S: 0.4 };
      const gestalt = foldAffectGestalt(previous, next);
      expect(gestalt.Cn).toBeCloseTo(0.438, 6);
      expect(gestalt.C).toBeCloseTo(0.638, 6);
      expect(gestalt.J).toBeCloseTo(0.662, 6);
    });

    it("loads and persists the affect gestalt incrementally", async () => {
      await withTempDir("openclaw-affect-gestalt-", async (root) => {
        const historyRoot = path.join(root, "memory", "history");
        const auditDir = path.join(historyRoot, "audit");
        const stateDir = path.join(root, "state");
        await fs.mkdir(auditDir, { recursive: true });
        await fs.mkdir(path.join(stateDir, "logs"), { recursive: true });

        await fs.writeFile(
          path.join(auditDir, "2026-03-14.jsonl"),
          JSON.stringify({
            ts: 1_700_000_001,
            channel: "telegram_dm",
            kind: "driver_turn",
            session_key: "agent:main:main",
            user_text: "zar asks",
            assistant_text: "We checked in.\n\n⋄⟨Cn:.5 C:.7 Ct:.8 I:.2 J:.6 A:.4 S:.6⟩",
          }) + "\n",
          "utf-8",
        );

        let gestalt = await loadAffectGestalt({ historyRoot, statePath: stateDir });
        expect(gestalt).toMatchObject({ Cn: 0.5, C: 0.7, J: 0.6 });

        const gestaltPath = resolveAffectGestaltPath({ historyRoot, statePath: stateDir });
        expect(await fs.readFile(gestaltPath, "utf-8")).toContain('"lastTs":1700000001000');

        await appendHeartbeatTurnLog(
          {
            ts: 1_700_000_002_000,
            sessionKey: "agent:main:main",
            status: "sent",
            userText: "Heartbeat",
            assistantText: "Heartbeat with feeling.\n\n⋄⟨Cn:.3 C:.5 Ct:.4 I:0 J:.8 A:.2 S:.4⟩",
          },
          stateDir,
        );

        gestalt = await loadAffectGestalt({ historyRoot, statePath: stateDir });
        expect(gestalt).toMatchObject({ Cn: 0.4, C: 0.6, J: 0.7 });

        const block = await loadAffectAggregateBlock({ historyRoot, statePath: stateDir });
        expect(block).toContain("Affect trace aggregate:");
        expect(block).toContain("Cn:.4");
        expect(block).toContain("J:.7");
      });
    });
  });
});
