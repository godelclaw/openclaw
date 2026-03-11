import fs from "node:fs/promises";
import path from "node:path";
import { DEFAULT_HEARTBEAT_FILENAME } from "../agents/workspace.js";
import { resolveHeartbeatPrompt } from "../auto-reply/heartbeat.js";
import type { OpenClawConfig } from "../config/config.js";
import { resolveStateDir, writeConfigFile } from "../config/config.js";
import {
  runVeriCoreHeartbeatSyncValidate,
  type VeriCoreBridgeOptions,
  type VeriCoreHeartbeatSyncContractResult,
  type VeriCoreHeartbeatSyncObservedInput,
} from "./impetus.js";

export const CANONICAL_HEARTBEAT_PROMPT =
  "Heartbeat. Read HEARTBEAT.md and follow it calmly. If there is no clear call to action, reply HEARTBEAT_OK.";

function normalizeError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.trim() || "heartbeat sync validation failed";
}

async function readFileIfExists(filePath: string): Promise<string | null> {
  try {
    return await fs.readFile(filePath, "utf-8");
  } catch (error) {
    const code = (error as { code?: string })?.code;
    if (code === "ENOENT") {
      return null;
    }
    throw error;
  }
}

export function resolveHeartbeatCanonicalPath(workspaceDir: string): string {
  return path.join(workspaceDir, DEFAULT_HEARTBEAT_FILENAME);
}

export function resolveHeartbeatMirrorPath(
  stateDir: string = resolveStateDir(process.env),
): string {
  return path.join(stateDir, "workspace", DEFAULT_HEARTBEAT_FILENAME);
}

export function resolveHeartbeatPromptObserved(
  rawPrompt?: string | null,
): Pick<
  VeriCoreHeartbeatSyncObservedInput,
  "prompt_uses_file_reference" | "prompt_mentions_legacy_gas"
> {
  const effectivePrompt = resolveHeartbeatPrompt(rawPrompt ?? undefined);
  return {
    prompt_uses_file_reference: /heartbeat\.md/i.test(effectivePrompt),
    prompt_mentions_legacy_gas: /gas\.md/i.test(effectivePrompt),
  };
}

export async function resolveHeartbeatSyncObservedInput(params: {
  cfg: OpenClawConfig;
  workspaceDir: string;
  stateDir?: string;
}): Promise<VeriCoreHeartbeatSyncObservedInput | null> {
  const canonicalPath = resolveHeartbeatCanonicalPath(params.workspaceDir);
  const mirrorPath = resolveHeartbeatMirrorPath(params.stateDir);
  const rawPrompt = params.cfg.agents?.defaults?.heartbeat?.prompt?.trim() ?? "";
  const [canonicalContent, mirrorContent] = await Promise.all([
    readFileIfExists(canonicalPath),
    readFileIfExists(mirrorPath),
  ]);

  if (!canonicalContent && !mirrorContent && !rawPrompt) {
    return null;
  }

  const promptObserved = resolveHeartbeatPromptObserved(rawPrompt);
  return {
    canonical_available: canonicalContent !== null,
    mirror_available: mirrorContent !== null,
    mirror_matches:
      canonicalContent !== null && mirrorContent !== null && canonicalContent === mirrorContent,
    ...promptObserved,
  };
}

export async function validateHeartbeatSyncContract(params: {
  cfg: OpenClawConfig;
  workspaceDir: string;
  stateDir?: string;
  options?: VeriCoreBridgeOptions;
}): Promise<VeriCoreHeartbeatSyncContractResult | null> {
  const observed = await resolveHeartbeatSyncObservedInput(params);
  if (!observed) {
    return null;
  }

  try {
    return await runVeriCoreHeartbeatSyncValidate(observed, {
      timeoutMs: 750,
      disableSpawnFallback: true,
      ...params.options,
    });
  } catch (error) {
    return {
      checked: false,
      contract_ok: false,
      ...observed,
      error: normalizeError(error),
    } satisfies VeriCoreHeartbeatSyncContractResult;
  }
}

export async function syncHeartbeatArtifacts(params: {
  cfg: OpenClawConfig;
  workspaceDir: string;
  stateDir?: string;
  warn?: (message: string) => void;
  writeConfig?: (cfg: OpenClawConfig) => Promise<void>;
}): Promise<{
  canonicalPath: string;
  mirrorPath: string;
  canonicalAvailable: boolean;
  mirrorUpdated: boolean;
  promptUpdated: boolean;
}> {
  const canonicalPath = resolveHeartbeatCanonicalPath(params.workspaceDir);
  const mirrorPath = resolveHeartbeatMirrorPath(params.stateDir);
  const canonicalContent = await readFileIfExists(canonicalPath);
  const writeConfig = params.writeConfig ?? writeConfigFile;
  let mirrorUpdated = false;
  let promptUpdated = false;

  if (canonicalContent !== null) {
    const existingMirror = await readFileIfExists(mirrorPath);
    if (existingMirror !== canonicalContent) {
      await fs.mkdir(path.dirname(mirrorPath), { recursive: true });
      await fs.writeFile(mirrorPath, canonicalContent, "utf-8");
      mirrorUpdated = true;
    }
  }

  const promptObserved = resolveHeartbeatPromptObserved(
    params.cfg.agents?.defaults?.heartbeat?.prompt,
  );
  const needsPromptRepair =
    !promptObserved.prompt_uses_file_reference || promptObserved.prompt_mentions_legacy_gas;
  if (needsPromptRepair) {
    const nextCfg = structuredClone(params.cfg);
    nextCfg.agents ??= {};
    nextCfg.agents.defaults ??= {};
    nextCfg.agents.defaults.heartbeat ??= {};
    nextCfg.agents.defaults.heartbeat.prompt = CANONICAL_HEARTBEAT_PROMPT;

    try {
      await writeConfig(nextCfg);
      params.cfg.agents ??= {};
      params.cfg.agents.defaults ??= {};
      params.cfg.agents.defaults.heartbeat ??= {};
      params.cfg.agents.defaults.heartbeat.prompt = CANONICAL_HEARTBEAT_PROMPT;
      promptUpdated = true;
    } catch (error) {
      params.warn?.(`heartbeat prompt repair failed: ${normalizeError(error)}`);
    }
  }

  return {
    canonicalPath,
    mirrorPath,
    canonicalAvailable: canonicalContent !== null,
    mirrorUpdated,
    promptUpdated,
  };
}
