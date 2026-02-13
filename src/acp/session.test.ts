import type { McpServer } from "@agentclientprotocol/sdk";
import { describe, expect, it, afterEach } from "vitest";
import { createInMemorySessionStore } from "./session.js";

describe("acp session manager", () => {
  const store = createInMemorySessionStore();

  afterEach(() => {
    store.clearAllSessionsForTest();
  });

  it("tracks active runs and clears on cancel", () => {
    const session = store.createSession({
      sessionKey: "acp:test",
      cwd: "/tmp",
    });
    const controller = new AbortController();
    store.setActiveRun(session.sessionId, "run-1", controller);

    expect(store.getSessionByRunId("run-1")?.sessionId).toBe(session.sessionId);

    const cancelled = store.cancelActiveRun(session.sessionId);
    expect(cancelled).toBe(true);
    expect(store.getSessionByRunId("run-1")).toBeUndefined();
  });

  it("stores ACP mcpServers in session state", () => {
    const mcpServers = [
      {
        name: "lean-lsp",
        command: "lean-lsp-mcp",
        args: ["--stdio"],
        env: [{ name: "LEAN_PATH", value: "/tmp/lean" }],
      },
    ] satisfies McpServer[];

    const session = store.createSession({
      sessionKey: "acp:test",
      cwd: "/tmp",
      mcpServers,
    });

    expect(session.mcpServers).toEqual(mcpServers);
    expect(store.getSession(session.sessionId)?.mcpServers).toEqual(mcpServers);
  });
});
