import { resolveStartupIdentityObserved } from "../agents/system-prompt-report.js";
import type {
  SessionStartupIdentityContract,
  SessionSystemPromptReport,
} from "../config/sessions/types.js";
import {
  runVeriCoreStartupIdentityValidate,
  type VeriCoreStartupIdentityObservedInput,
} from "./impetus.js";

function normalizeError(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.trim() || "startup identity validation failed";
}

export function resolveStartupIdentityObservedInput(
  report: Pick<SessionSystemPromptReport, "injectedWorkspaceFiles" | "startupIdentityObserved">,
): VeriCoreStartupIdentityObservedInput {
  return report.startupIdentityObserved ?? resolveStartupIdentityObserved(report);
}

export async function attachStartupIdentityContract(
  report: SessionSystemPromptReport,
): Promise<SessionSystemPromptReport> {
  const observed = resolveStartupIdentityObservedInput(report);
  report.startupIdentityObserved = observed;

  try {
    report.startupIdentityContract = await runVeriCoreStartupIdentityValidate(observed, {
      timeoutMs: 750,
      disableSpawnFallback: true,
    });
  } catch (error) {
    report.startupIdentityContract = {
      checked: false,
      contract_ok: false,
      ...observed,
      error: normalizeError(error),
    } satisfies SessionStartupIdentityContract;
  }

  return report;
}
