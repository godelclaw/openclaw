import type { AffectTrace } from "../infra/recent-turn-window.js";
import {
  runVeriCoreAffectValidate,
  type VeriCoreBridgeOptions,
  type VeriCoreAffectContractResult,
  type VeriCoreAffectObservedInput,
} from "./impetus.js";

const AFFECT_KEYS = ["Cn", "C", "Ct", "I", "J", "A", "S"] as const;
const AFFECT_GAMMA_SCALED = 69;

function scaleToInt(v: number): number {
  return Math.round(v * 100);
}

function traceToIntArray(trace: AffectTrace): number[] {
  return AFFECT_KEYS.map((k) => scaleToInt(trace[k]));
}

function normalizeError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.trim() || "affect validation failed";
}

export function buildAffectObservedInput(params: {
  prevGestalt: AffectTrace | null;
  observation: AffectTrace;
  result: AffectTrace;
}): VeriCoreAffectObservedInput {
  return {
    affect_gamma: AFFECT_GAMMA_SCALED,
    affect_prev: params.prevGestalt ? traceToIntArray(params.prevGestalt) : null,
    affect_obs: traceToIntArray(params.observation),
    affect_result: traceToIntArray(params.result),
  };
}

export async function validateAffectContractFromGestalt(params: {
  prevGestalt: AffectTrace | null;
  observation: AffectTrace;
  result: AffectTrace;
  options?: VeriCoreBridgeOptions;
}): Promise<VeriCoreAffectContractResult> {
  const observed = buildAffectObservedInput(params);

  try {
    return await runVeriCoreAffectValidate(observed, {
      timeoutMs: 750,
      disableSpawnFallback: true,
      ...params.options,
    });
  } catch (error) {
    return {
      checked: false,
      contract_ok: false,
      gamma_locked: false,
      all_bounded: false,
      fold_matches: false,
      expected_result: null,
      ...observed,
      error: normalizeError(error),
    } satisfies VeriCoreAffectContractResult;
  }
}
