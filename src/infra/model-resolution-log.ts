/**
 * Model resolution audit logging.
 *
 * Appends one JSONL line per run with configured vs resolved model/provider/profile.
 * Also maintains an atomic summary file for /status display.
 * All I/O is best-effort — logging failure never breaks a run.
 */

import fs from "node:fs";
import path from "node:path";

export interface ModelResolutionEntry {
  ts: string;
  runId: string;
  parentRunId?: string;
  kind: "primary" | "followup";
  sessionKey?: string;
  configuredPrimary: string;
  configuredFallbacks: string[];
  resolvedModel: string;
  resolvedProvider: string;
  resolvedProfile?: string;
  modelChanged: boolean;
  providerChanged: boolean;
  fallbackReason?: "timeout" | "rate_limit" | "billing" | "policy" | "auth" | "other";
  attempts: Array<{ provider: string; model: string; error: string; reason?: string }>;
  usage?: { input?: number; output?: number };
}

const LOG_FILENAME = "model-resolution.jsonl";
const SUMMARY_FILENAME = "last-model-resolution.json";

export async function appendModelResolutionLog(
  statePath: string | undefined,
  entry: ModelResolutionEntry,
): Promise<void> {
  if (!statePath) {
    return;
  }
  try {
    const logPath = path.join(statePath, LOG_FILENAME);
    await fs.promises.appendFile(logPath, JSON.stringify(entry) + "\n", "utf-8");
  } catch {
    // Best-effort: never break a run for logging.
  }
  try {
    const summaryPath = path.join(statePath, SUMMARY_FILENAME);
    const tmpPath = summaryPath + ".tmp";
    await fs.promises.writeFile(tmpPath, JSON.stringify(entry, null, 2) + "\n", "utf-8");
    await fs.promises.rename(tmpPath, summaryPath);
  } catch {
    // Best-effort.
  }
}

export function readLastModelResolution(
  statePath: string | undefined,
): ModelResolutionEntry | undefined {
  if (!statePath) {
    return undefined;
  }
  try {
    const summaryPath = path.join(statePath, SUMMARY_FILENAME);
    const raw = fs.readFileSync(summaryPath, "utf-8");
    return JSON.parse(raw) as ModelResolutionEntry;
  } catch {
    return undefined;
  }
}

/**
 * Classify fallback reason from the attempts array.
 */
export function classifyFallbackReason(
  attempts: Array<{ reason?: string; error?: string }>,
): ModelResolutionEntry["fallbackReason"] {
  for (const a of attempts) {
    const reason = a.reason?.toLowerCase();
    const error = a.error?.toLowerCase();
    const combined = [reason, error].filter(Boolean).join(" ");
    if (!combined) {
      continue;
    }
    if (combined.includes("timeout")) {
      return "timeout";
    }
    if (combined.includes("rate_limit") || combined.includes("rate limit")) {
      return "rate_limit";
    }
    if (
      combined.includes("billing") ||
      combined.includes("402") ||
      combined.includes("insufficient")
    ) {
      return "billing";
    }
    if (combined.includes("policy")) {
      return "policy";
    }
    if (combined.includes("auth")) {
      return "auth";
    }
  }
  return attempts.length > 0 ? "other" : undefined;
}

/**
 * In-memory dedup for fallback alerts. At most one alert per transition per 5 minutes.
 */
const lastAlertTimes = new Map<string, number>();
const ALERT_DEDUP_MS = 5 * 60 * 1000;

export function shouldSendFallbackAlert(fromRef: string, toRef: string): boolean {
  const key = `${fromRef}->${toRef}`;
  const now = Date.now();
  const last = lastAlertTimes.get(key);
  if (last && now - last < ALERT_DEDUP_MS) {
    return false;
  }
  lastAlertTimes.set(key, now);
  return true;
}

export function formatModelResolutionStatusLine(entry?: ModelResolutionEntry): string | undefined {
  if (!entry) {
    return undefined;
  }
  const resolvedRef = `${entry.resolvedProvider}/${entry.resolvedModel}`;
  const details: string[] = [];
  if (entry.resolvedProfile?.trim()) {
    details.push(`profile ${entry.resolvedProfile.trim()}`);
  }
  if (entry.providerChanged) {
    details.push(`provider fallback${entry.fallbackReason ? `: ${entry.fallbackReason}` : ""}`);
  } else if (entry.modelChanged) {
    details.push(`model fallback${entry.fallbackReason ? `: ${entry.fallbackReason}` : ""}`);
  }
  return `🧭 Last used: ${resolvedRef}${details.length ? ` (${details.join(" · ")})` : ""}`;
}

export function formatModelFallbackAlert(params: {
  configuredPrimary: string;
  resolvedProvider: string;
  resolvedModel: string;
  providerChanged: boolean;
  fallbackReason?: ModelResolutionEntry["fallbackReason"];
}): string {
  const resolved = `${params.resolvedProvider}/${params.resolvedModel}`;
  const prefix = params.providerChanged ? "⚠️ Provider fallback" : "ℹ️ Model fallback";
  const reason = params.fallbackReason ? ` (${params.fallbackReason})` : "";
  return `${prefix}: ${params.configuredPrimary} → ${resolved}${reason}`;
}
