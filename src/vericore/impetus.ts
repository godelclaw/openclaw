import { spawn } from "node:child_process";
import { createConnection } from "node:net";

import type { FinalizedMsgContext } from "../auto-reply/templating.js";

const DEFAULT_DECIDE_TIMEOUT_MS = 3000;
const DEFAULT_RUN_TIMEOUT_MS = 120000;

export type VeriCoreRoutePreference = "off" | "gate" | "driver" | "fallback";

export type VeriCoreStimulusInput = {
  channel: string;
  actor: string;
  content: string;
  timestamp: number;
  session_key?: string;
  route_preference?: VeriCoreRoutePreference;
};

export type VeriCoreStimulusDecision = {
  allow: boolean;
  context: string;
  channel: string;
  reason?: string | null;
};

export type VeriCoreRouteTarget = "control" | "driver" | "fallback";

export type VeriCoreStimulusRouteDecision = {
  allow: boolean;
  context: string;
  channel: string;
  route: VeriCoreRouteTarget;
  reason?: string | null;
  is_control: boolean;
  command?: string | null;
  session_key: string;
};

export type VeriCoreTurnOutcome = {
  response: string;
  tools_used: string[];
  prompt_tokens: number;
  completion_tokens: number;
  security_hold?: {
    gate: string;
    reason: string;
    held_content: string;
    mindlock_path?: string | null;
  } | null;
};

export type VeriCoreRunResult = {
  decision: VeriCoreStimulusDecision;
  route?: VeriCoreStimulusRouteDecision;
  outcome?: VeriCoreTurnOutcome;
};

export type VeriCoreMemoryStatus = {
  memory: {
    file: string;
    total_items: number;
    by_tier: Record<string, number>;
    by_type: Record<string, number>;
    missing_embeddings?: number;
    missing_source_dates?: number;
  };
  history: {
    root: string;
    daily_files: number;
    weekly_files: number;
    daily_merged_files: number;
  };
};

export type VeriCoreMemoryQueryHit = {
  id: number;
  score: number;
  tier: string;
  type: string;
  summary: string;
  categories: string[];
  source_date?: string | null;
  reinforcement_count: number;
};

export type VeriCoreMemoryQueryResult = {
  query: string;
  tier: string;
  hits: VeriCoreMemoryQueryHit[];
};

export type VeriCoreMemoryRefineResult = {
  refine: {
    before_items: number;
    after_items: number;
    removed_items: number;
    merged_groups: number;
    source_dates_filled: number;
    embedded_items: number;
    embed_requested: boolean;
  };
  memory: {
    file: string;
    total_items: number;
    by_tier: Record<string, number>;
    by_type: Record<string, number>;
  };
  save_error?: string | null;
};

export type VeriCoreMemorySetTierResult = {
  id: number;
  tier: string;
  changed: boolean;
  operator_approved: boolean;
};

export type VeriCoreMindlockPendingItem = {
  id: string;
  name: string;
  path: string;
  size_bytes: number;
  modified_ts: number;
  target_path?: string | null;
  reason?: string | null;
  reviewer_assessment?: {
    verdict: string;
    reason: string;
    reviewed_at?: number;
  } | null;
};

export type VeriCoreMindlockPendingResult = {
  mindlock_dir: string;
  pending_dir: string;
  count: number;
  items: VeriCoreMindlockPendingItem[];
  as_of_ts?: number;
};

export type VeriCoreMindlockStatusResult = {
  mindlock_dir: string;
  counts: {
    in: number;
    out: number;
    pending: number;
    rejected: number;
  };
  as_of_ts?: number;
};

export type VeriCoreMindlockListResult = {
  mindlock_dir: string;
  box: "in" | "out" | "pending" | "rejected";
  dir: string;
  count: number;
  items: VeriCoreMindlockPendingItem[];
  as_of_ts?: number;
};

export type VeriCoreMindlockDecisionResult = {
  id: string;
  status: "approved" | "rejected";
  source: string;
  target?: string;
  archived_artifact?: string;
  archived_meta?: string | null;
  rejected_artifact?: string;
  rejected_meta?: string | null;
};

type VeriCoreSocketResponse<T> = {
  ok: boolean;
  id?: string;
  result?: T;
  error?: string;
};

export type VeriCoreBridgeOptions = {
  binPath?: string;
  configPath?: string;
  socketPath?: string;
  timeoutMs?: number;
  disableSocket?: boolean;
  disableSpawnFallback?: boolean;
};

export function resolveVeriCoreBinaryPath(): string {
  const fromEnv = process.env.VERICORE_CORE_BIN?.trim();
  if (fromEnv) {
    return fromEnv;
  }
  return "/home/zarclaw/repos/godelclaw/vericore-core/target/release/vericore-core";
}

export function resolveVeriCoreConfigPath(): string {
  const fromEnv = process.env.VERICORE_CORE_CONFIG?.trim();
  if (fromEnv) {
    return fromEnv;
  }
  return "/home/zarclaw/repos/godelclaw/vericore-core/vericore.toml";
}

export function resolveVeriCoreSocketPath(): string {
  const fromEnv = process.env.VERICORE_CORE_SOCKET?.trim();
  if (fromEnv) {
    return fromEnv;
  }

  try {
    const uid = typeof process.getuid === "function" ? process.getuid() : 1001;
    return `/run/user/${uid}/vericore-core.sock`;
  } catch {
    return "/tmp/vericore-core.sock";
  }
}

function resolveVeriCoreRunTimeoutMs(): number {
  const raw = process.env.VERICORE_RUN_TIMEOUT_MS?.trim();
  if (raw) {
    const parsed = Number(raw);
    if (Number.isFinite(parsed) && parsed > 0) {
      return Math.floor(parsed);
    }
  }
  return DEFAULT_RUN_TIMEOUT_MS;
}

export function deriveVeriCoreChannel(ctx: FinalizedMsgContext): string {
  const source = String(ctx.OriginatingChannel ?? ctx.Surface ?? ctx.Provider ?? "").toLowerCase();
  const chatType = String(ctx.ChatType ?? "").toLowerCase();
  const sessionKey = String(ctx.SessionKey ?? "").toLowerCase();

  if (
    source === "internal" ||
    source === "heartbeat" ||
    source === "exec-event" ||
    source === "cron-event" ||
    source === "cron"
  ) {
    return "internal";
  }

  if (source === "terminal") {
    return "terminal";
  }

  if (source === "moltbook") {
    return "moltbook";
  }

  if (source === "telegram") {
    if (sessionKey.includes("agent:family") || sessionKey.includes(":family:")) {
      return "telegram_family";
    }
    if (chatType === "group" || chatType === "supergroup" || chatType === "channel") {
      return "telegram_public";
    }
    return "telegram_dm";
  }

  if (
    source === "api" ||
    source === "web" ||
    source === "webchat" ||
    source === "discord" ||
    source === "slack" ||
    source === "signal" ||
    source === "whatsapp" ||
    source === "matrix"
  ) {
    return "api";
  }

  return "api";
}

export function buildVeriCoreStimulusInput(ctx: FinalizedMsgContext): VeriCoreStimulusInput {
  const channel = deriveVeriCoreChannel(ctx);
  const actor =
    ctx.SenderId ??
    ctx.SenderUsername ??
    ctx.SenderName ??
    ctx.From ??
    ctx.SessionKey ??
    "unknown";
  const content =
    ctx.BodyForCommands ??
    ctx.CommandBody ??
    ctx.RawBody ??
    ctx.BodyForAgent ??
    ctx.Body ??
    "";
  const timestamp =
    typeof ctx.Timestamp === "number" && Number.isFinite(ctx.Timestamp)
      ? Math.floor(ctx.Timestamp)
      : Math.floor(Date.now() / 1000);

  return {
    channel,
    actor,
    content,
    timestamp,
    session_key: ctx.SessionKey,
  };
}

async function runVeriCoreSocketMethod<T>(
  method:
    | "health"
    | "decide"
    | "route"
    | "run"
    | "memory_status"
    | "memory_query"
    | "memory_refine"
    | "memory_set_tier"
    | "mindlock_pending"
    | "mindlock_status"
    | "mindlock_list"
    | "mindlock_approve"
    | "mindlock_reject",
  stimulus: VeriCoreStimulusInput | undefined,
  query: string | undefined,
  embed: boolean | undefined,
  options: VeriCoreBridgeOptions,
  extraPayload?: Record<string, unknown>,
): Promise<T> {
  const socketPath = options.socketPath ?? resolveVeriCoreSocketPath();
  const timeoutMs = options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS;
  const requestPayload = JSON.stringify({ method, stimulus, query, embed, ...extraPayload });

  return await new Promise<T>((resolve, reject) => {
    const socket = createConnection({ path: socketPath });
    let response = "";
    let settled = false;

    const finishError = (error: Error): void => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeoutHandle);
      reject(error);
    };

    const finishSuccess = (): void => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timeoutHandle);

      try {
        const parsed = JSON.parse(response) as VeriCoreSocketResponse<T>;
        if (!parsed.ok) {
          reject(
            new Error(`vericore socket ${method} failed: ${parsed.error ?? "unknown daemon error"}`),
          );
          return;
        }
        if (typeof parsed.result === "undefined") {
          reject(new Error(`vericore socket ${method} missing 'result' payload`));
          return;
        }
        resolve(parsed.result);
      } catch (error) {
        reject(
          new Error(
            `vericore socket ${method} returned invalid JSON: ${(error as Error).message}; response='${response.trim()}'`,
          ),
        );
      }
    };

    const timeoutHandle = setTimeout(() => {
      socket.destroy();
      finishError(new Error(`vericore socket ${method} timed out after ${timeoutMs}ms`));
    }, timeoutMs);

    socket.on("connect", () => {
      socket.end(requestPayload);
    });

    socket.on("data", (chunk: Buffer | string) => {
      response += chunk.toString();
    });

    socket.on("end", () => {
      finishSuccess();
    });

    socket.on("close", (hadError) => {
      if (!hadError && !settled) {
        finishSuccess();
      }
    });

    socket.on("error", (error) => {
      finishError(error);
    });
  });
}

function isRecoverableSocketError(error: unknown): boolean {
  const code = (error as NodeJS.ErrnoException | undefined)?.code;
  return (
    code === "ENOENT" ||
    code === "ECONNREFUSED" ||
    code === "ENOTSOCK" ||
    code === "ECONNRESET" ||
    code === "EPIPE" ||
    code === "ETIMEDOUT"
  );
}

async function runVeriCoreCommandViaProcess(
  command: "decide" | "route" | "run",
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions,
): Promise<string> {
  const binPath = options.binPath ?? resolveVeriCoreBinaryPath();
  const configPath = options.configPath ?? resolveVeriCoreConfigPath();
  const timeoutMs = options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS;

  return await new Promise<string>((resolve, reject) => {
    const child = spawn(binPath, [command, "--config", configPath], {
      stdio: ["pipe", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    let timedOut = false;

    const timeoutHandle = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);

    child.stdout.on("data", (chunk: Buffer | string) => {
      stdout += chunk.toString();
    });

    child.stderr.on("data", (chunk: Buffer | string) => {
      stderr += chunk.toString();
    });

    child.on("error", (error) => {
      clearTimeout(timeoutHandle);
      reject(error);
    });

    child.on("close", (code) => {
      clearTimeout(timeoutHandle);
      if (timedOut) {
        reject(new Error(`vericore ${command} timed out after ${timeoutMs}ms`));
        return;
      }
      if (code !== 0) {
        reject(
          new Error(
            `vericore ${command} failed (code=${code}): ${stderr.trim() || "no stderr output"}`,
          ),
        );
        return;
      }
      resolve(stdout);
    });

    child.stdin.end(JSON.stringify(stimulus));
  });
}

function parseJsonOutput<T>(stdout: string, method: string): T {
  try {
    return JSON.parse(stdout) as T;
  } catch (error) {
    throw new Error(
      `vericore ${method} returned invalid JSON: ${(error as Error).message}; stdout='${stdout.trim()}'`,
      { cause: error },
    );
  }
}

async function runVeriCoreStimulusDecisionViaSocket(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions,
): Promise<VeriCoreStimulusDecision> {
  return await runVeriCoreSocketMethod<VeriCoreStimulusDecision>(
    "decide",
    stimulus,
    undefined,
    undefined,
    options,
  );
}

async function runVeriCoreStimulusDecisionViaProcess(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreStimulusDecision> {
  const stdout = await runVeriCoreCommandViaProcess("decide", stimulus, {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  });
  return parseJsonOutput<VeriCoreStimulusDecision>(stdout, "decide");
}

async function runVeriCoreStimulusRouteViaSocket(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions,
): Promise<VeriCoreStimulusRouteDecision> {
  return await runVeriCoreSocketMethod<VeriCoreStimulusRouteDecision>(
    "route",
    stimulus,
    undefined,
    undefined,
    options,
  );
}

async function runVeriCoreStimulusRouteViaProcess(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreStimulusRouteDecision> {
  const stdout = await runVeriCoreCommandViaProcess("route", stimulus, {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  });
  return parseJsonOutput<VeriCoreStimulusRouteDecision>(stdout, "route");
}

async function runVeriCoreStimulusRunViaSocket(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions,
): Promise<VeriCoreRunResult> {
  const result = await runVeriCoreSocketMethod<VeriCoreRunResult>(
    "run",
    stimulus,
    undefined,
    undefined,
    options,
  );
  if (!result?.decision) {
    throw new Error("vericore socket run missing 'decision' payload");
  }
  return result;
}

async function runVeriCoreStimulusRunViaProcess(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions,
): Promise<VeriCoreRunResult> {
  const stdout = await runVeriCoreCommandViaProcess("run", stimulus, {
    ...options,
    timeoutMs: options.timeoutMs ?? resolveVeriCoreRunTimeoutMs(),
  });

  // Back-compat: CLI run may emit TurnOutcome only.
  const parsed = parseJsonOutput<VeriCoreRunResult | VeriCoreTurnOutcome>(stdout, "run");
  if ((parsed as VeriCoreRunResult).decision) {
    return parsed as VeriCoreRunResult;
  }

  const decision = await runVeriCoreStimulusDecisionViaProcess(stimulus, {
    ...options,
    timeoutMs: DEFAULT_DECIDE_TIMEOUT_MS,
  });
  return {
    decision,
    outcome: parsed as VeriCoreTurnOutcome,
  };
}

export async function runVeriCoreStimulusDecision(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreStimulusDecision> {
  const decideOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  if (!decideOptions.disableSocket) {
    try {
      return await runVeriCoreStimulusDecisionViaSocket(stimulus, decideOptions);
    } catch (error) {
      if (decideOptions.disableSpawnFallback || !isRecoverableSocketError(error)) {
        throw error;
      }
    }
  }

  return await runVeriCoreStimulusDecisionViaProcess(stimulus, decideOptions);
}

export async function runVeriCoreStimulusRoute(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreStimulusRouteDecision> {
  const routeOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  if (!routeOptions.disableSocket) {
    try {
      return await runVeriCoreStimulusRouteViaSocket(stimulus, routeOptions);
    } catch (error) {
      if (routeOptions.disableSpawnFallback || !isRecoverableSocketError(error)) {
        throw error;
      }
    }
  }

  return await runVeriCoreStimulusRouteViaProcess(stimulus, routeOptions);
}

export async function runVeriCoreStimulusRun(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreRunResult> {
  const runOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? resolveVeriCoreRunTimeoutMs(),
  };

  if (!runOptions.disableSocket) {
    try {
      return await runVeriCoreStimulusRunViaSocket(stimulus, runOptions);
    } catch (error) {
      if (runOptions.disableSpawnFallback || !isRecoverableSocketError(error)) {
        throw error;
      }
    }
  }

  return await runVeriCoreStimulusRunViaProcess(stimulus, runOptions);
}

export async function runVeriCoreMemoryStatus(
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMemoryStatus> {
  const statusOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMemoryStatus>(
    "memory_status",
    undefined,
    undefined,
    undefined,
    statusOptions,
  );
}

export async function runVeriCoreMemoryQuery(
  query: string,
  stimulus?: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMemoryQueryResult> {
  const queryOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMemoryQueryResult>(
    "memory_query",
    stimulus,
    query,
    undefined,
    queryOptions,
  );
}

export async function runVeriCoreMemoryRefine(
  embed = false,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMemoryRefineResult> {
  const refineOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMemoryRefineResult>(
    "memory_refine",
    undefined,
    undefined,
    embed,
    refineOptions,
  );
}

export async function runVeriCoreMemorySetTier(
  memoryId: number,
  tier: string,
  stimulus: VeriCoreStimulusInput,
  operatorApproved = false,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMemorySetTierResult> {
  const setTierOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMemorySetTierResult>(
    "memory_set_tier",
    stimulus,
    undefined,
    undefined,
    setTierOptions,
    {
      memory_id: memoryId,
      tier,
      operator_approved: operatorApproved,
    },
  );
}

export async function runVeriCoreMindlockStatus(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMindlockStatusResult> {
  const statusOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMindlockStatusResult>(
    "mindlock_status",
    stimulus,
    undefined,
    undefined,
    statusOptions,
  );
}

export async function runVeriCoreMindlockList(
  box: "in" | "out" | "pending" | "rejected",
  stimulus: VeriCoreStimulusInput,
  limit?: number,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMindlockListResult> {
  const listOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMindlockListResult>(
    "mindlock_list",
    stimulus,
    undefined,
    undefined,
    listOptions,
    {
      stage: box,
      limit,
    },
  );
}

export async function runVeriCoreMindlockPending(
  stimulus: VeriCoreStimulusInput,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMindlockPendingResult> {
  const pendingOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMindlockPendingResult>(
    "mindlock_pending",
    stimulus,
    undefined,
    undefined,
    pendingOptions,
  );
}

export async function runVeriCoreMindlockApprove(
  artifactId: string,
  stimulus: VeriCoreStimulusInput,
  reason?: string,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMindlockDecisionResult> {
  const approveOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMindlockDecisionResult>(
    "mindlock_approve",
    stimulus,
    undefined,
    undefined,
    approveOptions,
    {
      artifact_id: artifactId,
      reason,
    },
  );
}

export async function runVeriCoreMindlockReject(
  artifactId: string,
  stimulus: VeriCoreStimulusInput,
  reason?: string,
  options: VeriCoreBridgeOptions = {},
): Promise<VeriCoreMindlockDecisionResult> {
  const rejectOptions = {
    ...options,
    timeoutMs: options.timeoutMs ?? DEFAULT_DECIDE_TIMEOUT_MS,
  };

  return await runVeriCoreSocketMethod<VeriCoreMindlockDecisionResult>(
    "mindlock_reject",
    stimulus,
    undefined,
    undefined,
    rejectOptions,
    {
      artifact_id: artifactId,
      reason,
    },
  );
}

export async function runVeriCoreDecisionForContext(
  ctx: FinalizedMsgContext,
  options?: VeriCoreBridgeOptions,
): Promise<VeriCoreStimulusDecision> {
  return await runVeriCoreStimulusDecision(buildVeriCoreStimulusInput(ctx), options);
}

export async function runVeriCoreRouteForContext(
  ctx: FinalizedMsgContext,
  options?: VeriCoreBridgeOptions,
): Promise<VeriCoreStimulusRouteDecision> {
  return await runVeriCoreStimulusRoute(buildVeriCoreStimulusInput(ctx), options);
}

export async function runVeriCoreRunForContext(
  ctx: FinalizedMsgContext,
  options?: VeriCoreBridgeOptions,
): Promise<VeriCoreRunResult> {
  return await runVeriCoreStimulusRun(buildVeriCoreStimulusInput(ctx), options);
}
