import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { resolveStateDir } from "../config/paths.js";

export interface HeartbeatAuditEntry {
  ts: number;
  status: "sent" | "ok-empty" | "ok-token" | "skipped" | "failed";
  to?: string;
  accountId?: string;
  preview?: string;
  durationMs?: number;
  hasMedia?: boolean;
  reason?: string;
  channel?: string;
  silent?: boolean;
  indicatorType?: "ok" | "alert" | "error";
  agentId?: string;
  sessionKey?: string;
}

const LOG_FILENAME = "heartbeat-events.jsonl";
const SUMMARY_FILENAME = "last-heartbeat-event.json";

export function resolveHeartbeatAuditPaths(
  statePath: string = resolveStateDir(process.env),
): {
  logsDir: string;
  logPath: string;
  summaryPath: string;
} {
  const logsDir = path.join(statePath, "logs");
  return {
    logsDir,
    logPath: path.join(logsDir, LOG_FILENAME),
    summaryPath: path.join(logsDir, SUMMARY_FILENAME),
  };
}

export async function appendHeartbeatAuditLog(
  entry: HeartbeatAuditEntry,
  statePath: string = resolveStateDir(process.env),
): Promise<void> {
  const { logsDir, logPath, summaryPath } = resolveHeartbeatAuditPaths(statePath);
  try {
    await fsp.mkdir(logsDir, { recursive: true });
    await fsp.appendFile(logPath, JSON.stringify(entry) + "\n", "utf-8");
  } catch {
    // Best-effort: heartbeat logging must never break the agent.
  }
  try {
    await fsp.mkdir(logsDir, { recursive: true });
    const tmpPath = summaryPath + ".tmp";
    await fsp.writeFile(tmpPath, JSON.stringify(entry, null, 2) + "\n", "utf-8");
    await fsp.rename(tmpPath, summaryPath);
  } catch {
    // Best-effort.
  }
}

export async function readLastHeartbeatAudit(
  statePath: string = resolveStateDir(process.env),
): Promise<HeartbeatAuditEntry | undefined> {
  try {
    const { summaryPath } = resolveHeartbeatAuditPaths(statePath);
    const raw = await fsp.readFile(summaryPath, "utf-8");
    return JSON.parse(raw) as HeartbeatAuditEntry;
  } catch {
    return undefined;
  }
}

export function readLastHeartbeatAuditSync(
  statePath: string = resolveStateDir(process.env),
): HeartbeatAuditEntry | undefined {
  try {
    const { summaryPath } = resolveHeartbeatAuditPaths(statePath);
    const raw = fs.readFileSync(summaryPath, "utf-8");
    return JSON.parse(raw) as HeartbeatAuditEntry;
  } catch {
    return undefined;
  }
}
