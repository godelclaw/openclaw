import fs from "node:fs/promises";
import path from "node:path";
import { resolveAgentConfig, resolveAgentWorkspaceDir } from "../agents/agent-scope.js";
import { DEFAULT_GAS_FILENAME } from "../agents/workspace.js";
import type { OpenClawConfig } from "../config/config.js";
import { resolveStateDir } from "../config/paths.js";
import { createSubsystemLogger } from "../logging/subsystem.js";
import { normalizeAgentId } from "../routing/session-key.js";

type GasPeriod = "daily" | "weekly" | "monthly";

type GasUsage = {
  daily: number;
  weekly: number;
  monthly: number;
  total: number;
  limit: number | null;
  limitRemaining: number | null;
};

type GasAlertState = {
  dailyThreshold: number;
  weeklyThreshold: number;
  monthlyThreshold: number;
  dailyUsedUsd: number | null;
  dailyQuotaUsd: number | null;
  limitRemainingUsd: number | null;
  updatedAt?: string;
};

type ResolvedGasConfig = {
  quotas: Record<GasPeriod, number | null>;
  thresholds: number[];
  alertsEnabled: boolean;
  alertPeriods: Set<GasPeriod>;
};

export type GasThresholdAlert = {
  period: GasPeriod;
  threshold: number;
  usedUsd: number;
  quotaUsd: number;
  ratio: number;
};

export type GasRefreshResult = {
  usage: GasUsage;
  quotas: Record<GasPeriod, number | null>;
  thresholds: number[];
  alerts: GasThresholdAlert[];
  gasFilePath: string;
};

const DEFAULT_THRESHOLDS = [50, 75, 90, 100] as const;
const DEFAULT_ALERT_PERIODS: GasPeriod[] = ["daily", "weekly", "monthly"];
const OPENROUTER_AUTH_KEY_URL = "https://openrouter.ai/api/v1/auth/key";
const OPENROUTER_TIMEOUT_MS = 10_000;

const log = createSubsystemLogger("gas");

function toFiniteNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number(value.trim());
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return null;
}

function normalizeQuota(value: unknown): number | null {
  const parsed = toFiniteNumber(value);
  if (parsed === null || parsed <= 0) {
    return null;
  }
  return parsed;
}

function normalizeNonNegative(value: unknown): number | null {
  const parsed = toFiniteNumber(value);
  if (parsed === null || parsed < 0) {
    return null;
  }
  return parsed;
}

function normalizeThresholds(raw?: number[]): number[] {
  const input = Array.isArray(raw) && raw.length > 0 ? raw : [...DEFAULT_THRESHOLDS];
  const uniq = new Set<number>();
  for (const value of input) {
    const parsed = toFiniteNumber(value);
    if (parsed === null) {
      continue;
    }
    const clamped = Math.min(100, Math.max(1, Math.round(parsed)));
    uniq.add(clamped);
  }
  const normalized = [...uniq].sort((a, b) => a - b);
  return normalized.length > 0 ? normalized : [...DEFAULT_THRESHOLDS];
}

function normalizeAlertPeriods(raw?: Array<GasPeriod>): Set<GasPeriod> {
  const base = Array.isArray(raw) && raw.length > 0 ? raw : DEFAULT_ALERT_PERIODS;
  return new Set(base.filter((value): value is GasPeriod => value === "daily" || value === "weekly" || value === "monthly"));
}

function resolveGasConfig(cfg: OpenClawConfig, agentId: string): ResolvedGasConfig | null {
  const defaults = cfg.agents?.defaults?.gas;
  const overrides = resolveAgentConfig(cfg, agentId)?.gas;
  const merged = defaults || overrides ? { ...defaults, ...overrides } : undefined;
  if (!merged) {
    return null;
  }

  const quotas: Record<GasPeriod, number | null> = {
    daily: normalizeQuota(merged.dailyUsd),
    weekly: normalizeQuota(merged.weeklyUsd),
    monthly: normalizeQuota(merged.monthlyUsd),
  };
  const hasAnyQuota = Object.values(quotas).some((value) => value !== null);
  const enabled = merged.enabled ?? hasAnyQuota;
  if (!enabled) {
    return null;
  }

  return {
    quotas,
    thresholds: normalizeThresholds(merged.thresholds),
    alertsEnabled: merged.alerts?.enabled ?? true,
    alertPeriods: normalizeAlertPeriods(merged.alerts?.periods),
  };
}

async function fetchOpenRouterUsage(apiKey: string): Promise<GasUsage> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), OPENROUTER_TIMEOUT_MS);
  try {
    const response = await fetch(OPENROUTER_AUTH_KEY_URL, {
      method: "GET",
      headers: {
        Authorization: `Bearer ${apiKey}`,
      },
      signal: controller.signal,
    });
    if (!response.ok) {
      throw new Error(`OpenRouter usage request failed (${response.status})`);
    }

    const payload = (await response.json()) as { data?: Record<string, unknown> };
    const data = payload.data;
    if (!data || typeof data !== "object") {
      throw new Error("OpenRouter usage response missing data field");
    }

    return {
      daily: toFiniteNumber(data.usage_daily) ?? 0,
      weekly: toFiniteNumber(data.usage_weekly) ?? 0,
      monthly: toFiniteNumber(data.usage_monthly) ?? 0,
      total: toFiniteNumber(data.usage) ?? 0,
      limit: normalizeQuota(data.limit),
      limitRemaining: normalizeNonNegative(data.limit_remaining),
    };
  } finally {
    clearTimeout(timeout);
  }
}

function resolveStatePath(agentId: string): string {
  const stateDir = resolveStateDir(process.env);
  return path.join(stateDir, "gas", `${normalizeAgentId(agentId)}.json`);
}

async function loadAlertState(filePath: string): Promise<GasAlertState> {
  try {
    const raw = await fs.readFile(filePath, "utf-8");
    const parsed = JSON.parse(raw) as Partial<GasAlertState>;
    return {
      dailyThreshold: Number.isFinite(parsed.dailyThreshold) ? Number(parsed.dailyThreshold) : 0,
      weeklyThreshold: Number.isFinite(parsed.weeklyThreshold) ? Number(parsed.weeklyThreshold) : 0,
      monthlyThreshold: Number.isFinite(parsed.monthlyThreshold) ? Number(parsed.monthlyThreshold) : 0,
      dailyUsedUsd: normalizeNonNegative(parsed.dailyUsedUsd),
      dailyQuotaUsd: normalizeQuota(parsed.dailyQuotaUsd),
      limitRemainingUsd: normalizeNonNegative(parsed.limitRemainingUsd),
      updatedAt: typeof parsed.updatedAt === "string" ? parsed.updatedAt : undefined,
    };
  } catch {
    return {
      dailyThreshold: 0,
      weeklyThreshold: 0,
      monthlyThreshold: 0,
      dailyUsedUsd: null,
      dailyQuotaUsd: null,
      limitRemainingUsd: null,
      updatedAt: undefined,
    };
  }
}

async function saveAlertState(filePath: string, state: GasAlertState) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(state, null, 2)}\n`, "utf-8");
}

function resolveReachedThreshold(params: {
  usageUsd: number;
  quotaUsd: number | null;
  thresholds: number[];
}): number {
  if (!params.quotaUsd || params.quotaUsd <= 0) {
    return 0;
  }
  const percent = (params.usageUsd / params.quotaUsd) * 100;
  let reached = 0;
  for (const threshold of params.thresholds) {
    if (percent >= threshold) {
      reached = threshold;
    }
  }
  return reached;
}

function formatUsd(value: number, digits = 2): string {
  return value.toFixed(digits);
}

function formatUsageLine(params: { label: string; usageUsd: number; quotaUsd: number | null }): string {
  if (!params.quotaUsd) {
    return `- ${params.label}: $${formatUsd(params.usageUsd, 4)} (quota not set)`;
  }
  const ratio = params.usageUsd / params.quotaUsd;
  const percent = Math.round(ratio * 100);
  return `- ${params.label}: $${formatUsd(params.usageUsd, 4)} / $${formatUsd(params.quotaUsd)} (${percent}%)`;
}

function buildGasMarkdown(params: {
  usage: GasUsage;
  quotas: Record<GasPeriod, number | null>;
  thresholds: number[];
  alerts: GasThresholdAlert[];
}): string {
  const lines: string[] = [];
  lines.push(`# ${DEFAULT_GAS_FILENAME}`);
  lines.push(`Updated: ${new Date().toISOString()} | OpenRouter`);
  lines.push(`Today: $${formatUsd(params.usage.daily, 4)} | Week: $${formatUsd(params.usage.weekly, 4)} | Month: $${formatUsd(params.usage.monthly, 4)} | Total: $${formatUsd(params.usage.total, 4)}`);
  if (params.usage.limit) {
    const remaining = params.usage.limitRemaining ?? 0;
    lines.push(`Key limit: $${formatUsd(params.usage.limit)} (remaining $${formatUsd(remaining, 4)})`);
  }
  return lines.join("\n");
}

function resolveCreditsLeftUsd(state: GasAlertState): number | null {
  if (typeof state.limitRemainingUsd === "number" && Number.isFinite(state.limitRemainingUsd)) {
    return Math.max(0, state.limitRemainingUsd);
  }
  if (state.dailyQuotaUsd !== null && state.dailyUsedUsd !== null) {
    return Math.max(0, state.dailyQuotaUsd - state.dailyUsedUsd);
  }
  return null;
}

export async function resolveGasCreditsTag(params: {
  cfg: OpenClawConfig;
  agentId: string;
}): Promise<string | undefined> {
  const config = resolveGasConfig(params.cfg, params.agentId);
  if (!config) {
    return undefined;
  }
  const state = await loadAlertState(resolveStatePath(params.agentId));
  if (typeof state.dailyUsedUsd === "number" && Number.isFinite(state.dailyUsedUsd)) {
    return `[$${formatUsd(state.dailyUsedUsd)} today]`;
  }
  return undefined;
}

export function formatGasAlertMessage(alerts: GasThresholdAlert[] | undefined): string | undefined {
  if (!alerts || alerts.length === 0) {
    return undefined;
  }

  const parts = alerts.map((alert) => {
    const period = alert.period[0].toUpperCase() + alert.period.slice(1);
    const percent = Math.round(alert.ratio * 100);
    const base = `${period} gas reached: ${percent}%`;
    return `${base}.`;
  });

  return parts.join(" ");
}

export async function refreshAgentGasStatus(params: {
  cfg: OpenClawConfig;
  agentId: string;
}): Promise<GasRefreshResult | null> {
  const config = resolveGasConfig(params.cfg, params.agentId);
  if (!config) {
    return null;
  }

  const apiKey = process.env.OPENROUTER_API_KEY?.trim();
  if (!apiKey) {
    return null;
  }

  let usage: GasUsage;
  try {
    usage = await fetchOpenRouterUsage(apiKey);
  } catch (err) {
    log.warn(`gas: failed to fetch OpenRouter usage (${String(err)})`);
    return null;
  }

  const statePath = resolveStatePath(params.agentId);
  const previous = await loadAlertState(statePath);
  const next: GasAlertState = { ...previous };
  const alerts: GasThresholdAlert[] = [];

  const usageByPeriod: Record<GasPeriod, number> = {
    daily: usage.daily,
    weekly: usage.weekly,
    monthly: usage.monthly,
  };

  const previousByPeriod: Record<GasPeriod, number> = {
    daily: previous.dailyThreshold,
    weekly: previous.weeklyThreshold,
    monthly: previous.monthlyThreshold,
  };

  for (const period of ["daily", "weekly", "monthly"] as const) {
    const quotaUsd = config.quotas[period];
    const usedUsd = usageByPeriod[period];
    const reached = resolveReachedThreshold({
      usageUsd: usedUsd,
      quotaUsd,
      thresholds: config.thresholds,
    });

    if (period === "daily") {
      next.dailyThreshold = reached;
    } else if (period === "weekly") {
      next.weeklyThreshold = reached;
    } else {
      next.monthlyThreshold = reached;
    }

    if (
      quotaUsd &&
      reached > previousByPeriod[period] &&
      config.alertsEnabled &&
      config.alertPeriods.has(period)
    ) {
      alerts.push({
        period,
        threshold: reached,
        usedUsd,
        quotaUsd,
        ratio: usedUsd / quotaUsd,
      });
    }
  }

  next.dailyUsedUsd = usage.daily;
  next.dailyQuotaUsd = config.quotas.daily;
  next.limitRemainingUsd = usage.limitRemaining;
  next.updatedAt = new Date().toISOString();

  await saveAlertState(statePath, next);

  const workspaceDir = resolveAgentWorkspaceDir(params.cfg, params.agentId);
  await fs.mkdir(workspaceDir, { recursive: true });
  const gasFilePath = path.join(workspaceDir, DEFAULT_GAS_FILENAME);
  const gasMarkdown = buildGasMarkdown({
    usage,
    quotas: config.quotas,
    thresholds: config.thresholds,
    alerts,
  });
  await fs.writeFile(gasFilePath, `${gasMarkdown}\n`, "utf-8");

  return {
    usage,
    quotas: config.quotas,
    thresholds: config.thresholds,
    alerts,
    gasFilePath,
  };
}
