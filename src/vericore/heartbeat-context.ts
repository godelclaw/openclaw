import { resolveStateDir } from "../config/paths.js";
import {
  DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT,
  DEFAULT_HEARTBEAT_TRACE_LIMIT,
  loadRecentHeartbeatTrace,
  loadRecentTurnCandidates,
  selectRecentTurns,
  type HeartbeatTurnLogEntry,
  type RecentTurnWindowEntry,
} from "../infra/recent-turn-window.js";
import {
  runVeriCoreHeartbeatContextValidate,
  type VeriCoreBridgeOptions,
  type VeriCoreHeartbeatContextContractResult,
  type VeriCoreHeartbeatContextObservedInput,
} from "./impetus.js";

function normalizeError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.trim() || "heartbeat context validation failed";
}

function isOldestFirst(entries: Array<{ ts: number }>): boolean {
  for (let i = 1; i < entries.length; i += 1) {
    if (entries[i - 1]!.ts > entries[i]!.ts) {
      return false;
    }
  }
  return true;
}

export function buildHeartbeatContextObservedInput(params: {
  recentTurnCandidates: RecentTurnWindowEntry[];
  selectedRecentTurns: RecentTurnWindowEntry[];
  heartbeatTrace: HeartbeatTurnLogEntry[];
  recentTurnLimit: number;
  heartbeatTraceLimit: number;
}): VeriCoreHeartbeatContextObservedInput {
  return {
    recent_turn_count: params.selectedRecentTurns.length,
    recent_turn_limit: params.recentTurnLimit,
    reserved_candidate_present: params.recentTurnCandidates.some((entry) => entry.reserveMainTelegram),
    reserved_included: params.selectedRecentTurns.some((entry) => entry.reserveMainTelegram),
    content_heartbeat_candidate_present: params.recentTurnCandidates.some(
      (entry) => entry.source === "heartbeat" && !entry.noopHeartbeat,
    ),
    content_heartbeat_included: params.selectedRecentTurns.some(
      (entry) => entry.source === "heartbeat" && !entry.noopHeartbeat,
    ),
    noop_heartbeat_in_recent_turns: params.selectedRecentTurns.some((entry) => entry.noopHeartbeat),
    heartbeat_trace_count: params.heartbeatTrace.length,
    heartbeat_trace_limit: params.heartbeatTraceLimit,
    heartbeat_trace_oldest_first: isOldestFirst(params.heartbeatTrace),
    heartbeat_trace_only_heartbeat_entries: true,
  };
}

export async function resolveHeartbeatContextObservedInput(params: {
  historyRoot: string;
  mainSessionKey?: string;
  statePath?: string;
  recentTurnLimit?: number;
  heartbeatTraceLimit?: number;
  scanLimit?: number;
}): Promise<VeriCoreHeartbeatContextObservedInput> {
  const statePath = params.statePath ?? resolveStateDir(process.env);
  const recentTurnLimit = Math.max(1, params.recentTurnLimit ?? DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT);
  const heartbeatTraceLimit = Math.max(
    0,
    params.heartbeatTraceLimit ?? DEFAULT_HEARTBEAT_TRACE_LIMIT,
  );
  const scanLimit = Math.max(
    params.scanLimit ?? 64,
    recentTurnLimit,
    heartbeatTraceLimit > 0 ? heartbeatTraceLimit : 1,
  );
  const recentTurnCandidates = await loadRecentTurnCandidates({
    historyRoot: params.historyRoot,
    statePath,
    mainSessionKey: params.mainSessionKey,
    limit: recentTurnLimit,
    scanLimit,
  });
  const selectedRecentTurns = selectRecentTurns({
    entries: recentTurnCandidates,
    limit: recentTurnLimit,
  });
  const heartbeatTrace =
    heartbeatTraceLimit > 0
      ? await loadRecentHeartbeatTrace({ statePath, scanLimit })
      : [];
  return buildHeartbeatContextObservedInput({
    recentTurnCandidates,
    selectedRecentTurns,
    heartbeatTrace,
    recentTurnLimit,
    heartbeatTraceLimit,
  });
}

export async function validateHeartbeatContextContract(params: {
  historyRoot: string;
  mainSessionKey?: string;
  statePath?: string;
  recentTurnLimit?: number;
  heartbeatTraceLimit?: number;
  scanLimit?: number;
  options?: VeriCoreBridgeOptions;
}): Promise<VeriCoreHeartbeatContextContractResult> {
  const observed = await resolveHeartbeatContextObservedInput(params);
  try {
    return await runVeriCoreHeartbeatContextValidate(observed, {
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
    } satisfies VeriCoreHeartbeatContextContractResult;
  }
}

export async function validateHeartbeatContextContractFromData(params: {
  recentTurnCandidates: RecentTurnWindowEntry[];
  selectedRecentTurns: RecentTurnWindowEntry[];
  heartbeatTrace: HeartbeatTurnLogEntry[];
  recentTurnLimit: number;
  heartbeatTraceLimit: number;
  options?: VeriCoreBridgeOptions;
}): Promise<VeriCoreHeartbeatContextContractResult> {
  const observed = buildHeartbeatContextObservedInput(params);
  try {
    return await runVeriCoreHeartbeatContextValidate(observed, {
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
    } satisfies VeriCoreHeartbeatContextContractResult;
  }
}
