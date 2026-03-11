import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  applyRepoStateDirFallback,
  detectRepoStateDirFromCwd,
  rewriteUpdateFlagArgv,
  shouldEnsureCliPath,
  shouldRegisterPrimarySubcommand,
  shouldSkipPluginCommandRegistration,
} from "./run-main.js";

describe("rewriteUpdateFlagArgv", () => {
  it("leaves argv unchanged when --update is absent", () => {
    const argv = ["node", "entry.js", "status"];
    expect(rewriteUpdateFlagArgv(argv)).toBe(argv);
  });

  it("rewrites --update into the update command", () => {
    expect(rewriteUpdateFlagArgv(["node", "entry.js", "--update"])).toEqual([
      "node",
      "entry.js",
      "update",
    ]);
  });

  it("preserves global flags that appear before --update", () => {
    expect(rewriteUpdateFlagArgv(["node", "entry.js", "--profile", "p", "--update"])).toEqual([
      "node",
      "entry.js",
      "--profile",
      "p",
      "update",
    ]);
  });

  it("keeps update options after the rewritten command", () => {
    expect(rewriteUpdateFlagArgv(["node", "entry.js", "--update", "--json"])).toEqual([
      "node",
      "entry.js",
      "update",
      "--json",
    ]);
  });
});

describe("shouldRegisterPrimarySubcommand", () => {
  it("skips eager primary registration for help/version invocations", () => {
    expect(shouldRegisterPrimarySubcommand(["node", "openclaw", "status", "--help"])).toBe(false);
    expect(shouldRegisterPrimarySubcommand(["node", "openclaw", "-V"])).toBe(false);
    expect(shouldRegisterPrimarySubcommand(["node", "openclaw", "-v"])).toBe(false);
  });

  it("keeps eager primary registration for regular command runs", () => {
    expect(shouldRegisterPrimarySubcommand(["node", "openclaw", "status"])).toBe(true);
    expect(shouldRegisterPrimarySubcommand(["node", "openclaw", "acp", "-v"])).toBe(true);
  });
});

describe("shouldSkipPluginCommandRegistration", () => {
  it("skips plugin registration for root help/version", () => {
    expect(
      shouldSkipPluginCommandRegistration({
        argv: ["node", "openclaw", "--help"],
        primary: null,
        hasBuiltinPrimary: false,
      }),
    ).toBe(true);
  });

  it("skips plugin registration for builtin subcommand help", () => {
    expect(
      shouldSkipPluginCommandRegistration({
        argv: ["node", "openclaw", "config", "--help"],
        primary: "config",
        hasBuiltinPrimary: true,
      }),
    ).toBe(true);
  });

  it("skips plugin registration for builtin command runs", () => {
    expect(
      shouldSkipPluginCommandRegistration({
        argv: ["node", "openclaw", "sessions", "--json"],
        primary: "sessions",
        hasBuiltinPrimary: true,
      }),
    ).toBe(true);
  });

  it("keeps plugin registration for non-builtin help", () => {
    expect(
      shouldSkipPluginCommandRegistration({
        argv: ["node", "openclaw", "voicecall", "--help"],
        primary: "voicecall",
        hasBuiltinPrimary: false,
      }),
    ).toBe(false);
  });

  it("keeps plugin registration for non-builtin command runs", () => {
    expect(
      shouldSkipPluginCommandRegistration({
        argv: ["node", "openclaw", "voicecall", "status"],
        primary: "voicecall",
        hasBuiltinPrimary: false,
      }),
    ).toBe(false);
  });
});

describe("shouldEnsureCliPath", () => {
  it("skips path bootstrap for help/version invocations", () => {
    expect(shouldEnsureCliPath(["node", "openclaw", "--help"])).toBe(false);
    expect(shouldEnsureCliPath(["node", "openclaw", "-V"])).toBe(false);
    expect(shouldEnsureCliPath(["node", "openclaw", "-v"])).toBe(false);
  });

  it("skips path bootstrap for read-only fast paths", () => {
    expect(shouldEnsureCliPath(["node", "openclaw", "status"])).toBe(false);
    expect(shouldEnsureCliPath(["node", "openclaw", "--log-level", "debug", "status"])).toBe(false);
    expect(shouldEnsureCliPath(["node", "openclaw", "sessions", "--json"])).toBe(false);
    expect(shouldEnsureCliPath(["node", "openclaw", "config", "get", "update"])).toBe(false);
    expect(shouldEnsureCliPath(["node", "openclaw", "models", "status", "--json"])).toBe(false);
  });

  it("keeps path bootstrap for mutating or unknown commands", () => {
    expect(shouldEnsureCliPath(["node", "openclaw", "message", "send"])).toBe(true);
    expect(shouldEnsureCliPath(["node", "openclaw", "voicecall", "status"])).toBe(true);
    expect(shouldEnsureCliPath(["node", "openclaw", "acp", "-v"])).toBe(true);
  });
});

describe("repo state dir fallback", () => {
  async function withTempRoot(prefix: string, run: (root: string) => Promise<void>): Promise<void> {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), prefix));
    try {
      await run(root);
    } finally {
      await fs.rm(root, { recursive: true, force: true });
    }
  }

  it("detects the nearest repo-local .BGIseed-state from a nested cwd", async () => {
    await withTempRoot("openclaw-repo-state-", async (root) => {
      const nested = path.join(root, "src", "cli");
      await fs.mkdir(path.join(root, ".BGIseed-state"), { recursive: true });
      await fs.mkdir(nested, { recursive: true });
      await fs.writeFile(path.join(root, ".BGIseed-state", "openclaw.json"), "{}", "utf-8");

      expect(detectRepoStateDirFromCwd(nested)).toBe(path.join(root, ".BGIseed-state"));
    });
  });

  it("sets OPENCLAW_STATE_DIR when repo state exists and no explicit override is present", async () => {
    await withTempRoot("openclaw-repo-state-", async (root) => {
      await fs.mkdir(path.join(root, ".BGIseed-state"), { recursive: true });
      await fs.writeFile(path.join(root, ".BGIseed-state", "openclaw.json"), "{}", "utf-8");

      const env = {} as NodeJS.ProcessEnv;
      expect(applyRepoStateDirFallback(env, root)).toBe(path.join(root, ".BGIseed-state"));
      expect(env.OPENCLAW_STATE_DIR).toBe(path.join(root, ".BGIseed-state"));
    });
  });

  it("keeps explicit state/config/home overrides authoritative", () => {
    const stateDir = "/explicit/state";
    expect(
      applyRepoStateDirFallback({ OPENCLAW_STATE_DIR: stateDir } as NodeJS.ProcessEnv, "/tmp"),
    ).toBeNull();
    expect(
      applyRepoStateDirFallback({ OPENCLAW_CONFIG_PATH: "/explicit/openclaw.json" } as NodeJS.ProcessEnv, "/tmp"),
    ).toBeNull();
    expect(
      applyRepoStateDirFallback({ OPENCLAW_HOME: "/explicit/home" } as NodeJS.ProcessEnv, "/tmp"),
    ).toBeNull();
  });
});
