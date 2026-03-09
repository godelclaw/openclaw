/**
 * Tool broker server for VeriCore.
 *
 * Exposes MCP tools over a Unix socket so the Rust agent driver
 * can list and call tools without owning MCP lifecycle.
 *
 * Protocol: same length-prefixed JSON framing as completions bridge.
 * Policy: Rust has already authorized the call through the 6-gate chain.
 * This server just executes.
 */

import { createServer, type Server } from "node:net";
import fs from "node:fs";
import os from "node:os";
import crypto from "node:crypto";

import type { OpenClawConfig } from "../config/config.js";
import { resolveSessionAgent } from "./session-resolver.js";
import { initMcpRuntime, type McpRuntime, type McpToolHandle } from "../agents/mcp-client.js";
import { resolveAgentMcpServers } from "../agents/agent-scope.js";
import {
  consumeAdjustedParamsForToolCall,
  wrapToolWithBeforeToolCallHook,
} from "../agents/pi-tools.before-tool-call.js";
import { resolveToolLoopDetectionConfig } from "../agents/pi-tools.js";
import { jsonResult, type AnyAgentTool } from "../agents/tools/common.js";
import { createSubsystemLogger } from "../logging/subsystem.js";
import { isPlainObject } from "../utils.js";

/**
 * Native VeriCore tool names that MCP tools must not shadow.
 * If an MCP tool has a colliding name, initMcpRuntime will uniquify it
 * (e.g., "exec" → "exec-2"), but the capabilityId stays stable.
 */
const NATIVE_TOOL_NAMES = new Set([
  "read_file",
  "write_file",
  "list_dir",
  "exec",
  "web_fetch",
  "promote_from_mindlock",
  "request_review",
  "self_escalate",
]);

const log = createSubsystemLogger("vericore/tool-broker");

// ── Wire types ───────────────────────────────────────────────────────────

type ToolBrokerRequest = {
  request_id: string;
  session_key?: string | null;
  call_kind: "list_tools" | "call_tool";
  capability_id?: string;
  arguments?: Record<string, unknown>;
  audit_id?: number;
};

type ToolDescriptor = {
  capability_id: string;
  name: string;
  description: string;
  parameters: Record<string, unknown>;
  source: "mcp";
  mcp_server: string;
};

type ToolBrokerResponse = {
  ok: boolean;
  error?: string;
  tools?: ToolDescriptor[];
  result?: string;
  is_error?: boolean;
  execution_ms?: number;
};

// ── Per-agent MCP registry ───────────────────────────────────────────────

type CatalogEntry = {
  runtime: McpRuntime;
  tools: McpToolHandle[];
  configHash: string;
  lastUsed: number;
};

class ToolBrokerRegistry {
  private catalogs = new Map<string, CatalogEntry>();
  private readonly idleTimeoutMs: number;

  constructor(opts?: { idleTimeoutMs?: number }) {
    this.idleTimeoutMs = opts?.idleTimeoutMs ?? 10 * 60 * 1000; // 10 min default
  }

  async getCatalog(
    agentId: string,
    mcpServers: unknown[] | undefined,
  ): Promise<{ tools: McpToolHandle[] }> {
    const configHash = crypto
      .createHash("sha256")
      .update(JSON.stringify(mcpServers ?? []))
      .digest("hex")
      .slice(0, 16);

    const existing = this.catalogs.get(agentId);
    if (existing && existing.configHash === configHash) {
      existing.lastUsed = Date.now();
      return { tools: existing.tools };
    }

    // Config changed or first init — tear down old runtime if any
    if (existing) {
      log.info(`MCP config changed for agent ${agentId}, reinitializing`);
      await existing.runtime.cleanup().catch((err) => {
        log.warn(`cleanup failed for agent ${agentId}: ${String(err)}`);
      });
    }

    log.info(`initializing MCP runtime for agent ${agentId} (hash: ${configHash})`);
    const runtime = await initMcpRuntime({
      mcpServers: mcpServers,
      existingToolNames: NATIVE_TOOL_NAMES,
      toolCallTimeoutMs: 60_000,
    });

    const entry: CatalogEntry = {
      runtime,
      tools: runtime.tools,
      configHash,
      lastUsed: Date.now(),
    };
    this.catalogs.set(agentId, entry);

    log.info(`agent ${agentId}: ${runtime.tools.length} MCP tools available`);
    return { tools: runtime.tools };
  }

  findTool(agentId: string, capabilityId: string): McpToolHandle | undefined {
    const entry = this.catalogs.get(agentId);
    if (!entry) return undefined;
    return entry.tools.find((t) => t.capabilityId === capabilityId);
  }

  async cleanupIdle(): Promise<void> {
    const now = Date.now();
    for (const [agentId, entry] of this.catalogs) {
      if (now - entry.lastUsed > this.idleTimeoutMs) {
        log.info(`reaping idle MCP runtime for agent ${agentId}`);
        await entry.runtime.cleanup().catch(() => {});
        this.catalogs.delete(agentId);
      }
    }
  }

  async cleanupAll(): Promise<void> {
    for (const [agentId, entry] of this.catalogs) {
      await entry.runtime.cleanup().catch(() => {});
      this.catalogs.delete(agentId);
    }
  }
}

// ── Length-prefixed framing (same as completions bridge) ──────────────────

function readLengthPrefixedMessage(data: Buffer): { payload: Buffer; remaining: Buffer } | null {
  if (data.length < 4) return null;
  const len = data.readUInt32BE(0);
  if (data.length < 4 + len) return null;
  return {
    payload: data.subarray(4, 4 + len),
    remaining: data.subarray(4 + len),
  };
}

function writeLengthPrefixedMessage(payload: Buffer): Buffer {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(payload.length, 0);
  return Buffer.concat([header, payload]);
}

function stringifyToolPayload(payload: unknown): string {
  if (typeof payload === "string") {
    return payload;
  }
  try {
    const encoded = JSON.stringify(payload, null, 2);
    if (typeof encoded === "string") {
      return encoded;
    }
  } catch {
    // Fall through to String(payload) for non-serializable values.
  }
  return String(payload);
}

function extractToolResultText(result: unknown): string {
  if (!result || typeof result !== "object") {
    return stringifyToolPayload(result);
  }

  const record = result as { content?: unknown[]; details?: unknown };
  if (Array.isArray(record.content)) {
    const textParts = record.content
      .filter(
        (entry): entry is { type: string; text: string } =>
          typeof entry === "object" &&
          entry !== null &&
          "type" in entry &&
          "text" in entry &&
          (entry as { type?: unknown }).type === "text" &&
          typeof (entry as { text?: unknown }).text === "string",
      )
      .map((entry) => entry.text);
    if (textParts.length > 0) {
      return textParts.join("\n");
    }
  }

  return stringifyToolPayload("details" in record ? record.details : record);
}

// ── Request handlers ─────────────────────────────────────────────────────

async function handleListTools(
  req: ToolBrokerRequest,
  cfg: OpenClawConfig,
  registry: ToolBrokerRegistry,
): Promise<ToolBrokerResponse> {
  const { agentId } = resolveSessionAgent({
    sessionKey: req.session_key,
    cfg,
  });

  const rawMcpServers = resolveAgentMcpServers(cfg, agentId);
  const { tools } = await registry.getCatalog(agentId, rawMcpServers);

  const descriptors: ToolDescriptor[] = tools.map((t) => {
    // Normalize schema: ensure top-level type:"object" for provider compatibility,
    // and strip known-problematic keywords. This is a conservative normalization
    // since we don't know the final LLM provider at list time.
    let schema = t.inputSchema ?? { type: "object", properties: {} };
    if (schema && typeof schema === "object" && !("type" in schema)) {
      schema = { type: "object", ...schema };
    }
    return {
      capability_id: t.capabilityId,
      name: t.name,
      description: t.description ?? `MCP tool from ${t.serverName}`,
      parameters: schema,
      source: "mcp" as const,
      mcp_server: t.serverName,
    };
  });

  return { ok: true, tools: descriptors };
}

async function handleCallTool(
  req: ToolBrokerRequest,
  cfg: OpenClawConfig,
  registry: ToolBrokerRegistry,
): Promise<ToolBrokerResponse> {
  const capabilityId = req.capability_id;
  if (!capabilityId) {
    return { ok: false, error: "missing capability_id" };
  }

  const { agentId } = resolveSessionAgent({
    sessionKey: req.session_key,
    cfg,
  });

  // Ensure catalog is initialized
  const rawMcpServers = resolveAgentMcpServers(cfg, agentId);
  await registry.getCatalog(agentId, rawMcpServers);

  const tool = registry.findTool(agentId, capabilityId);
  if (!tool) {
    return {
      ok: true,
      result: `Error: tool ${capabilityId} not found in agent ${agentId} catalog`,
      is_error: true,
    };
  }

  const args = req.arguments ?? {};
  const toolCallId = req.request_id;
  const start = Date.now();
  const wrappedTool = wrapToolWithBeforeToolCallHook(
    {
      name: tool.name,
      label: tool.name,
      description: tool.description ?? `MCP tool from ${tool.serverName}`,
      parameters: tool.inputSchema ?? { type: "object", properties: {} },
      execute: async (_callId, params, signal) => {
        const callArgs = isPlainObject(params) ? params : {};
        const result = await tool.call(callArgs, {
          timeoutMs: 60_000,
          signal,
        });
        return jsonResult(result);
      },
    } satisfies AnyAgentTool,
    {
      agentId,
      sessionKey: req.session_key ?? undefined,
      loopDetection: resolveToolLoopDetectionConfig({ cfg, agentId }),
    },
  );

  try {
    const result = await wrappedTool.execute(toolCallId, args, undefined);
    const elapsed = Date.now() - start;
    consumeAdjustedParamsForToolCall(toolCallId);
    const resultStr = extractToolResultText(result);

    log.info(`tool ${capabilityId} executed in ${elapsed}ms (audit_id=${req.audit_id ?? "none"})`);

    return {
      ok: true,
      result: resultStr,
      is_error: false,
      execution_ms: elapsed,
    };
  } catch (err) {
    const elapsed = Date.now() - start;
    const errMsg = err instanceof Error ? err.message : String(err);
    consumeAdjustedParamsForToolCall(toolCallId);

    log.warn(`tool ${capabilityId} failed after ${elapsed}ms: ${errMsg}`);

    return {
      ok: true,
      result: `Error: ${errMsg}`,
      is_error: true,
      execution_ms: elapsed,
    };
  }
}

// ── Server ───────────────────────────────────────────────────────────────

export function startToolBrokerServer(params: {
  cfg: OpenClawConfig;
}): { server: Server; socketPath: string; cleanup: () => void } {
  const uid = os.userInfo().uid;
  const socketPath = `/run/user/${uid}/openclaw-toolbroker.sock`;
  const registry = new ToolBrokerRegistry();

  // Clean up stale socket
  try {
    fs.unlinkSync(socketPath);
  } catch {
    // ignore if doesn't exist
  }

  // Periodic idle cleanup (every 5 min)
  const cleanupInterval = setInterval(() => {
    void registry.cleanupIdle();
  }, 5 * 60 * 1000);

  const server = createServer({ allowHalfOpen: true }, (conn) => {
    let buffer = Buffer.alloc(0);
    let processing = false;

    const tryProcess = () => {
      if (processing) return;
      const parsed = readLengthPrefixedMessage(buffer);
      if (!parsed) return;
      processing = true;

      void (async () => {
        try {
          const req: ToolBrokerRequest = JSON.parse(parsed.payload.toString("utf-8"));
          log.info(`broker request ${req.request_id} kind=${req.call_kind} session=${req.session_key ?? "none"}`);

          let response: ToolBrokerResponse;
          switch (req.call_kind) {
            case "list_tools":
              response = await handleListTools(req, params.cfg, registry);
              break;
            case "call_tool":
              response = await handleCallTool(req, params.cfg, registry);
              break;
            default:
              response = { ok: false, error: `unknown call_kind: ${String(req.call_kind)}` };
          }

          const payload = Buffer.from(JSON.stringify(response));
          conn.write(writeLengthPrefixedMessage(payload));
          conn.end();
        } catch (err) {
          const errMsg = err instanceof Error ? err.message : String(err);
          log.error(`broker request failed: ${errMsg}`);

          try {
            const errResp: ToolBrokerResponse = { ok: false, error: errMsg };
            const payload = Buffer.from(JSON.stringify(errResp));
            conn.write(writeLengthPrefixedMessage(payload));
            conn.end();
          } catch {
            // connection may already be closed
          }
        }
      })();
    };

    conn.on("data", (chunk: Buffer) => {
      buffer = Buffer.concat([buffer, chunk]);
      tryProcess();
    });

    conn.on("end", () => {
      tryProcess();
      if (!processing) {
        const errResp: ToolBrokerResponse = { ok: false, error: "incomplete or missing length prefix" };
        const payload = Buffer.from(JSON.stringify(errResp));
        conn.write(writeLengthPrefixedMessage(payload));
        conn.end();
      }
    });

    conn.on("error", (err) => {
      log.warn(`broker connection error: ${err.message}`);
    });
  });

  server.listen(socketPath, () => {
    try {
      fs.chmodSync(socketPath, 0o600);
    } catch {
      // non-fatal
    }
    log.info(`tool broker listening on ${socketPath}`);
  });

  server.on("error", (err) => {
    log.error(`tool broker server error: ${err.message}`);
  });

  const cleanup = () => {
    clearInterval(cleanupInterval);
    server.close();
    void registry.cleanupAll();
    try {
      fs.unlinkSync(socketPath);
    } catch {
      // ignore
    }
  };

  return { server, socketPath, cleanup };
}
