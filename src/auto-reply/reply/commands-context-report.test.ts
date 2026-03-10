import { describe, expect, it } from "vitest";
import { buildContextReply } from "./commands-context-report.js";
import type { HandleCommandsParams } from "./commands-types.js";

function makeParams(
  commandBodyNormalized: string,
  truncated: boolean,
  options?: { omitBootstrapLimits?: boolean },
): HandleCommandsParams {
  return {
    command: {
      commandBodyNormalized,
      channel: "telegram",
      senderIsOwner: true,
    },
    sessionKey: "agent:default:main",
    workspaceDir: "/tmp/workspace",
    contextTokens: null,
    provider: "openai",
    model: "gpt-5",
    elevated: { allowed: false },
    resolvedThinkLevel: "off",
    resolvedReasoningLevel: "off",
    sessionEntry: {
      totalTokens: 123,
      inputTokens: 100,
      outputTokens: 23,
      systemPromptReport: {
        source: "run",
        generatedAt: Date.now(),
        workspaceDir: "/tmp/workspace",
        bootstrapMaxChars: options?.omitBootstrapLimits ? undefined : 20_000,
        bootstrapTotalMaxChars: options?.omitBootstrapLimits ? undefined : 150_000,
        sandbox: { mode: "off", sandboxed: false },
        systemPrompt: {
          chars: 1_000,
          projectContextChars: 500,
          nonProjectContextChars: 500,
        },
        injectedWorkspaceFiles: [
          {
            name: "AGENTS.md",
            path: "/tmp/workspace/AGENTS.md",
            missing: false,
            rawChars: truncated ? 200_000 : 10_000,
            injected: true,
            injectedChars: truncated ? 20_000 : 10_000,
            truncated,
          },
          {
            name: "SOUL.md",
            path: "/tmp/workspace/SOUL.md",
            missing: false,
            rawChars: 5_000,
            injected: true,
            injectedChars: 5_000,
            truncated: false,
          },
        ],
        startupIdentityObserved: {
          agents_available: true,
          agents_injected: true,
          soul_available: true,
          soul_injected: true,
        },
        startupIdentityContract: {
          checked: true,
          contract_ok: true,
          agents_available: true,
          agents_injected: true,
          soul_available: true,
          soul_injected: true,
        },
        skills: {
          promptChars: 10,
          entries: [{ name: "checks", blockChars: 10 }],
        },
        tools: {
          listChars: 10,
          schemaChars: 20,
          entries: [{ name: "read", summaryChars: 10, schemaChars: 20, propertiesCount: 1 }],
        },
      },
    },
    cfg: {},
    ctx: {},
    commandBody: "",
    commandArgs: [],
    resolvedElevatedLevel: "off",
  } as unknown as HandleCommandsParams;
}

describe("buildContextReply", () => {
  it("shows bootstrap truncation warning in list output when context exceeds configured limits", async () => {
    const result = await buildContextReply(makeParams("/context list", true));
    expect(result.text).toContain("Bootstrap max/total: 150,000 chars");
    expect(result.text).toContain("⚠ Bootstrap context is over configured limits");
    expect(result.text).toContain("Causes: 1 file(s) exceeded max/file.");
  });

  it("does not show bootstrap truncation warning when there is no truncation", async () => {
    const result = await buildContextReply(makeParams("/context list", false));
    expect(result.text).not.toContain("Bootstrap context is over configured limits");
  });

  it("shows startup identity lines when present in the report", async () => {
    const result = await buildContextReply(makeParams("/context list", false));
    expect(result.text).toContain(
      "Startup identity (observed): AGENTS available=true injected=true; SOUL available=true injected=true",
    );
    expect(result.text).toContain("Startup identity (VeriCore): verified");
  });

  it("falls back to config defaults when legacy reports are missing bootstrap limits", async () => {
    const result = await buildContextReply(
      makeParams("/context list", false, {
        omitBootstrapLimits: true,
      }),
    );
    expect(result.text).toContain("Bootstrap max/file: 20,000 chars");
    expect(result.text).toContain("Bootstrap max/total: 150,000 chars");
    expect(result.text).not.toContain("Bootstrap max/file: ? chars");
  });
});
