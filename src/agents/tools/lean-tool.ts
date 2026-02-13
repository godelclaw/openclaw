import { Type } from "@sinclair/typebox";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { runCommandWithTimeout } from "../../process/exec.js";
import { assertSandboxPath } from "../sandbox-paths.js";
import { stringEnum } from "../schema/typebox.js";
import { type AnyAgentTool, jsonResult, readStringParam } from "./common.js";

const LEAN_TOOL_ACTIONS = ["version", "check"] as const;
const DEFAULT_TIMEOUT_MS = 20_000;
const MIN_TIMEOUT_MS = 1_000;
const MAX_TIMEOUT_MS = 120_000;

const LeanToolSchema = Type.Object({
  action: stringEnum(LEAN_TOOL_ACTIONS),
  filePath: Type.Optional(
    Type.String({
      description:
        "Lean file path (relative to workspace). Required for action=check unless code is set.",
    }),
  ),
  code: Type.Optional(
    Type.String({
      description: "Lean source snippet to check. A temporary file is created when provided.",
    }),
  ),
  timeoutMs: Type.Optional(
    Type.Number({
      description: `Command timeout in ms (${MIN_TIMEOUT_MS}-${MAX_TIMEOUT_MS}, default ${DEFAULT_TIMEOUT_MS}).`,
    }),
  ),
  useLake: Type.Optional(
    Type.Boolean({
      description:
        "When true, run via lake env lean. When omitted, auto-detect lakefile.lean/lakefile.toml.",
    }),
  ),
  keepFile: Type.Optional(
    Type.Boolean({
      description: "When code is provided, keep the generated scratch file on disk.",
    }),
  ),
});

type CommandResult = {
  ok: boolean;
  code: number | null;
  signal: NodeJS.Signals | null;
  killed: boolean;
  stdout: string;
  stderr: string;
  error?: string;
};

function resolveTimeoutMs(raw: unknown): number {
  const value =
    typeof raw === "number" && Number.isFinite(raw) ? Math.floor(raw) : DEFAULT_TIMEOUT_MS;
  return Math.max(MIN_TIMEOUT_MS, Math.min(MAX_TIMEOUT_MS, value));
}

function dedupePathEntries(entries: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const entry of entries) {
    const trimmed = entry.trim();
    if (!trimmed || seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    out.push(trimmed);
  }
  return out;
}

function resolveLeanPathEnv(): string {
  const entries: string[] = [];
  const currentPath = process.env.PATH ?? "";
  if (currentPath) {
    entries.push(...currentPath.split(path.delimiter));
  }

  const home = os.homedir();
  if (home) {
    entries.push(path.join(home, ".elan", "bin"));
    entries.push(path.join(home, ".local", "bin"));
  }

  return dedupePathEntries(entries).join(path.delimiter);
}

async function pathExists(filePath: string): Promise<boolean> {
  try {
    await fs.access(filePath);
    return true;
  } catch {
    return false;
  }
}

function pathWithinRoot(candidate: string, root: string): boolean {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

async function resolveLakeRoot(params: {
  startDir: string;
  workspaceRoot: string;
}): Promise<string | null> {
  const workspaceRoot = path.resolve(params.workspaceRoot);
  let current = path.resolve(params.startDir);

  if (!pathWithinRoot(current, workspaceRoot)) {
    return null;
  }

  while (true) {
    const hasLakefile =
      (await pathExists(path.join(current, "lakefile.lean"))) ||
      (await pathExists(path.join(current, "lakefile.toml")));
    if (hasLakefile) {
      return current;
    }

    if (current === workspaceRoot) {
      return null;
    }
    const parent = path.dirname(current);
    if (parent === current || !pathWithinRoot(parent, workspaceRoot)) {
      return null;
    }
    current = parent;
  }
}

async function runToolCommand(
  argv: string[],
  options: { cwd: string; timeoutMs: number; pathEnv: string },
) {
  try {
    const result = await runCommandWithTimeout(argv, {
      cwd: options.cwd,
      timeoutMs: options.timeoutMs,
      env: { PATH: options.pathEnv },
    });
    return {
      ok: result.code === 0,
      code: result.code,
      signal: result.signal,
      killed: result.killed,
      stdout: result.stdout,
      stderr: result.stderr,
    } satisfies CommandResult;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      ok: false,
      code: null,
      signal: null,
      killed: false,
      stdout: "",
      stderr: "",
      error: message,
    } satisfies CommandResult;
  }
}

function createScratchFilePath(workspaceRoot: string): string {
  const stamp = `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`;
  return path.join(workspaceRoot, ".openclaw", "lean", "scratch", `snippet-${stamp}.lean`);
}

export function createLeanTool(options?: { workspaceRoot?: string }): AnyAgentTool {
  const workspaceRoot = path.resolve(options?.workspaceRoot?.trim() || process.cwd());

  return {
    label: "Lean",
    name: "lean",
    description:
      "Lean proof assistant helper. Use action=check to type-check Lean code/files (auto-detects Lake projects). Use action=version to inspect lean/lake availability.",
    parameters: LeanToolSchema,
    execute: async (_toolCallId, args) => {
      const params = args as Record<string, unknown>;
      const action = readStringParam(params, "action", { required: true });
      const timeoutMs = resolveTimeoutMs(params.timeoutMs);
      const pathEnv = resolveLeanPathEnv();

      if (action === "version") {
        const lean = await runToolCommand(["lean", "--version"], {
          cwd: workspaceRoot,
          timeoutMs: Math.min(timeoutMs, 15_000),
          pathEnv,
        });
        const lake = await runToolCommand(["lake", "--version"], {
          cwd: workspaceRoot,
          timeoutMs: Math.min(timeoutMs, 15_000),
          pathEnv,
        });
        return jsonResult({
          ok: lean.ok && lake.ok,
          action,
          workspaceRoot,
          lean,
          lake,
        });
      }

      if (action !== "check") {
        throw new Error(`Unsupported lean action: ${action}`);
      }

      const filePath = readStringParam(params, "filePath");
      const code = readStringParam(params, "code", { trim: false });
      const keepFile = typeof params.keepFile === "boolean" ? params.keepFile : false;

      if ((filePath && code) || (!filePath && !code)) {
        throw new Error("Provide exactly one of filePath or code for action=check.");
      }

      let targetPath = "";
      let temporaryFile: string | undefined;

      if (filePath) {
        const resolved = await assertSandboxPath({
          filePath,
          cwd: workspaceRoot,
          root: workspaceRoot,
        });
        targetPath = resolved.resolved;
      } else {
        const scratchPath = createScratchFilePath(workspaceRoot);
        await fs.mkdir(path.dirname(scratchPath), { recursive: true });
        await fs.writeFile(scratchPath, code ?? "", "utf8");
        targetPath = scratchPath;
        temporaryFile = scratchPath;
      }

      const explicitUseLake = typeof params.useLake === "boolean" ? params.useLake : undefined;
      const lakeRoot = await resolveLakeRoot({
        startDir: path.dirname(targetPath),
        workspaceRoot,
      });
      const useLake = explicitUseLake === undefined ? Boolean(lakeRoot) : explicitUseLake;

      if (useLake && !lakeRoot) {
        throw new Error(
          "useLake=true requested, but no lakefile.lean/lakefile.toml found under workspace.",
        );
      }

      const argv = useLake ? ["lake", "env", "lean", targetPath] : ["lean", targetPath];
      const commandCwd = useLake ? (lakeRoot ?? workspaceRoot) : workspaceRoot;

      let result: CommandResult;
      try {
        result = await runToolCommand(argv, {
          cwd: commandCwd,
          timeoutMs,
          pathEnv,
        });
      } finally {
        if (temporaryFile && !keepFile) {
          await fs.rm(temporaryFile, { force: true }).catch(() => undefined);
        }
      }

      return jsonResult({
        ok: result.ok,
        action,
        command: argv.join(" "),
        workspaceRoot,
        cwd: commandCwd,
        useLake,
        lakeRoot,
        timeoutMs,
        targetPath,
        temporaryFile: keepFile ? temporaryFile : null,
        result,
      });
    },
  };
}
