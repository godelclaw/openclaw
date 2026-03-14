import {
  DEFAULT_ENERGY_CONVERSATION_GAP_MS,
  summarizeEnergySignals,
  type RecentTurnWindowEntry,
} from "../infra/recent-turn-window.js";
import {
  runVeriCoreEnergyValidate,
  type VeriCoreBridgeOptions,
  type VeriCoreEnergyContractResult,
  type VeriCoreEnergyObservedInput,
} from "./impetus.js";

function normalizeError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.trim() || "energy validation failed";
}

export function buildEnergyObservedInput(params: {
  entries: RecentTurnWindowEntry[];
  conversationGapMs?: number;
}): VeriCoreEnergyObservedInput {
  const summary = summarizeEnergySignals({
    entries: params.entries,
    conversationGapMs:
      params.conversationGapMs ?? DEFAULT_ENERGY_CONVERSATION_GAP_MS,
  });

  return {
    noop_heartbeat_count: summary.noopHeartbeatCount,
    acted_heartbeat_count: summary.actedHeartbeatCount,
    conversation_burst_count: summary.conversationBurstCount,
    conversation_gap_ms: summary.conversationGapMs,
    energy_value: summary.energy,
  };
}

export async function validateEnergyContractFromEntries(params: {
  entries: RecentTurnWindowEntry[];
  conversationGapMs?: number;
  options?: VeriCoreBridgeOptions;
}): Promise<VeriCoreEnergyContractResult> {
  const observed = buildEnergyObservedInput(params);

  try {
    return await runVeriCoreEnergyValidate(observed, {
      timeoutMs: 750,
      disableSpawnFallback: true,
      ...params.options,
    });
  } catch (error) {
    return {
      checked: false,
      contract_ok: false,
      expected_energy:
        observed.noop_heartbeat_count -
        observed.acted_heartbeat_count -
        observed.conversation_burst_count,
      gap_locked: false,
      energy_matches: false,
      ...observed,
      error: normalizeError(error),
    } satisfies VeriCoreEnergyContractResult;
  }
}
