import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";

const runCommandWithTimeoutMock = vi.fn();

vi.mock("../../process/exec.js", () => ({
  runCommandWithTimeout: (...args: unknown[]) => runCommandWithTimeoutMock(...args),
}));

import { createLeanTool } from "./lean-tool.js";

async function createTempRoot() {
  return await fs.mkdtemp(path.join(os.tmpdir(), "openclaw-lean-tool-"));
}

describe("lean tool", () => {
  beforeEach(() => {
    runCommandWithTimeoutMock.mockReset();
  });

  it("returns version info for lean and lake", async () => {
    runCommandWithTimeoutMock
      .mockResolvedValueOnce({
        stdout: "Lean (version 4.27.0)",
        stderr: "",
        code: 0,
        signal: null,
        killed: false,
      })
      .mockResolvedValueOnce({
        stdout: "Lake version 5.0.0",
        stderr: "",
        code: 0,
        signal: null,
        killed: false,
      });

    const workspaceRoot = await createTempRoot();
    try {
      const tool = createLeanTool({ workspaceRoot });
      const result = await tool.execute("call1", { action: "version" });
      const details = result.details as {
        ok: boolean;
        lean: { ok: boolean; stdout: string };
        lake: { ok: boolean; stdout: string };
      };

      expect(details.ok).toBe(true);
      expect(details.lean.ok).toBe(true);
      expect(details.lake.ok).toBe(true);
      expect(details.lean.stdout).toContain("Lean");
      expect(details.lake.stdout).toContain("Lake");

      expect(runCommandWithTimeoutMock).toHaveBeenCalledTimes(2);
      const leanArgv = runCommandWithTimeoutMock.mock.calls[0]?.[0] as string[];
      const lakeArgv = runCommandWithTimeoutMock.mock.calls[1]?.[0] as string[];
      expect(leanArgv).toEqual(["lean", "--version"]);
      expect(lakeArgv).toEqual(["lake", "--version"]);
    } finally {
      await fs.rm(workspaceRoot, { recursive: true, force: true }).catch(() => undefined);
    }
  });

  it("checks inline Lean code and removes scratch file", async () => {
    runCommandWithTimeoutMock.mockResolvedValueOnce({
      stdout: "",
      stderr: "",
      code: 0,
      signal: null,
      killed: false,
    });

    const workspaceRoot = await createTempRoot();
    try {
      const tool = createLeanTool({ workspaceRoot });
      const result = await tool.execute("call2", {
        action: "check",
        code: "#eval 1 + 1",
      });
      const details = result.details as {
        ok: boolean;
        command: string;
        targetPath: string;
        temporaryFile: string | null;
        result: { code: number | null };
      };

      expect(details.ok).toBe(true);
      expect(details.command.startsWith("lean ")).toBe(true);
      expect(details.targetPath.endsWith(".lean")).toBe(true);
      expect(details.temporaryFile).toBeNull();
      expect(details.result.code).toBe(0);

      const argv = runCommandWithTimeoutMock.mock.calls[0]?.[0] as string[];
      const options = runCommandWithTimeoutMock.mock.calls[0]?.[1] as {
        cwd?: string;
        env?: Record<string, string>;
      };
      expect(argv[0]).toBe("lean");
      expect(argv[1]).toBe(details.targetPath);
      expect(options.cwd).toBe(workspaceRoot);
      expect(options.env?.PATH).toContain(path.join(os.homedir(), ".elan", "bin"));

      await expect(fs.stat(details.targetPath)).rejects.toMatchObject({ code: "ENOENT" });
    } finally {
      await fs.rm(workspaceRoot, { recursive: true, force: true }).catch(() => undefined);
    }
  });

  it("auto-selects lake env when a lakefile exists", async () => {
    runCommandWithTimeoutMock.mockResolvedValueOnce({
      stdout: "",
      stderr: "",
      code: 0,
      signal: null,
      killed: false,
    });

    const workspaceRoot = await createTempRoot();
    try {
      await fs.writeFile(path.join(workspaceRoot, "lakefile.lean"), "import Lake\n", "utf8");
      await fs.mkdir(path.join(workspaceRoot, "proofs"), { recursive: true });
      await fs.writeFile(path.join(workspaceRoot, "proofs", "Demo.lean"), "def x := 1\n", "utf8");

      const tool = createLeanTool({ workspaceRoot });
      const result = await tool.execute("call3", {
        action: "check",
        filePath: "proofs/Demo.lean",
      });
      const details = result.details as {
        ok: boolean;
        useLake: boolean;
        lakeRoot: string | null;
      };

      expect(details.ok).toBe(true);
      expect(details.useLake).toBe(true);
      expect(details.lakeRoot).toBe(workspaceRoot);

      const argv = runCommandWithTimeoutMock.mock.calls[0]?.[0] as string[];
      const options = runCommandWithTimeoutMock.mock.calls[0]?.[1] as {
        cwd?: string;
      };
      expect(argv[0]).toBe("lake");
      expect(argv[1]).toBe("env");
      expect(argv[2]).toBe("lean");
      expect(argv[3]).toBe(path.join(workspaceRoot, "proofs", "Demo.lean"));
      expect(options.cwd).toBe(workspaceRoot);
    } finally {
      await fs.rm(workspaceRoot, { recursive: true, force: true }).catch(() => undefined);
    }
  });

  it("rejects paths outside the workspace", async () => {
    const workspaceRoot = await createTempRoot();
    try {
      const tool = createLeanTool({ workspaceRoot });
      await expect(
        tool.execute("call4", {
          action: "check",
          filePath: "../outside.lean",
        }),
      ).rejects.toThrow(/Path escapes sandbox root/);
    } finally {
      await fs.rm(workspaceRoot, { recursive: true, force: true }).catch(() => undefined);
    }
  });
});
