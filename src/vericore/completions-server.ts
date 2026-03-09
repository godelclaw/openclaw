import { createServer, type Server } from "node:net";
import fs from "node:fs";
import os from "node:os";

import {
  complete,
  type Api,
  type AssistantMessage as PiAssistantMessage,
  type Context as PiContext,
  type Message as PiMessage,
  type Model,
  type Tool as PiTool,
  type ToolCall as PiToolCall,
  type UserMessage,
  type ToolResultMessage,
} from "@mariozechner/pi-ai";

import type { OpenClawConfig } from "../config/config.js";
import { resolveSessionAgent } from "./session-resolver.js";
import { resolveStorePath } from "../config/sessions.js";
import { DEFAULT_MODEL, DEFAULT_PROVIDER } from "../agents/defaults.js";
import { resolveConfiguredModelRef } from "../agents/model-selection.js";
import { resolveRunModelFallbacksOverride } from "../agents/agent-scope.js";
import { runWithModelFallback } from "../agents/model-fallback.js";
import { resolveModel } from "../agents/pi-embedded-runner/model.js";
import { getApiKeyForModel } from "../agents/model-auth.js";
import { normalizeToolParameters } from "../agents/pi-tools.schema.js";
import type { AnyAgentTool } from "../agents/tools/common.js";
import { resolveStoredModelOverride } from "../auto-reply/reply/model-selection.js";
import { loadSessionStore } from "../config/sessions/store.js";
import {
  appendModelResolutionLog,
  classifyFallbackReason,
  formatModelFallbackAlert,
  shouldSendFallbackAlert,
} from "../infra/model-resolution-log.js";
import { createSubsystemLogger } from "../logging/subsystem.js";
import { resolveOperatorTelegramChatId, sendBridgeFallbackAlert } from "./operator-alert.js";

const log = createSubsystemLogger("vericore/completions");

// ── Bridge wire types (matching Rust GatewayLlmClient) ───────────────────

type BridgeRequest = {
  request_id: string;
  session_key?: string | null;
  call_kind: "driver_turn" | "security_review" | "memory_refine";
  prompt_mode: "driver" | "reviewer";
  messages: OpenAIMessage[];
  tools?: OpenAIToolDef[];
  max_tokens: number;
  temperature: number;
};

type OpenAIMessage = {
  role: "system" | "user" | "assistant" | "tool";
  content?: string | null;
  tool_calls?: Array<{
    id: string;
    type: string;
    function: { name: string; arguments: string };
  }>;
  tool_call_id?: string;
  name?: string;
};

type OpenAIToolDef = {
  type: "function";
  function: {
    name: string;
    description?: string;
    parameters?: Record<string, unknown>;
  };
};

type BridgeResponse = {
  ok: boolean;
  error?: string;
  configured_model?: string;
  resolved_model?: string;
  resolved_provider?: string;
  resolved_profile?: string;
  fallback_used?: boolean;
  provider_changed?: boolean;
  choices?: Array<{
    message: {
      content?: string | null;
      tool_calls?: Array<{
        id: string;
        function: { name: string; arguments: string };
      }>;
    };
  }>;
  usage?: { prompt_tokens: number; completion_tokens: number };
};

// ── OpenAI ↔ pi-ai format conversion ────────────────────────────────────

function convertOpenAIMessagesToPiAi(messages: OpenAIMessage[]): {
  systemPrompt?: string;
  messages: PiMessage[];
} {
  const systemParts: string[] = [];
  const piMessages: PiMessage[] = [];
  const now = Date.now();

  for (const msg of messages) {
    switch (msg.role) {
      case "system":
        if (msg.content) systemParts.push(msg.content);
        break;

      case "user":
        piMessages.push({
          role: "user",
          content: msg.content ?? "",
          timestamp: now,
        } as UserMessage);
        break;

      case "assistant": {
        const content: (PiToolCall | { type: "text"; text: string })[] = [];
        if (msg.content) {
          content.push({ type: "text", text: msg.content });
        }
        if (msg.tool_calls) {
          for (const tc of msg.tool_calls) {
            let parsed: Record<string, unknown> = {};
            try {
              parsed = JSON.parse(tc.function.arguments);
            } catch {
              // keep empty object on parse failure
            }
            content.push({
              type: "toolCall",
              id: tc.id,
              name: tc.function.name,
              arguments: parsed,
            } as PiToolCall);
          }
        }
        piMessages.push({
          role: "assistant",
          content,
          api: "openai-completions",
          provider: "bridge",
          model: "bridge",
          usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
          stopReason: "stop",
          timestamp: now,
        } as PiAssistantMessage);
        break;
      }

      case "tool":
        piMessages.push({
          role: "toolResult",
          toolCallId: msg.tool_call_id ?? "",
          toolName: msg.name ?? "",
          content: [{ type: "text", text: msg.content ?? "" }],
          isError: false,
          timestamp: now,
        } as ToolResultMessage);
        break;
    }
  }

  return {
    systemPrompt: systemParts.length > 0 ? systemParts.join("\n\n") : undefined,
    messages: piMessages,
  };
}

function convertOpenAIToolsToPiAi(
  tools: OpenAIToolDef[],
  options?: { modelProvider?: string; modelId?: string },
): PiTool[] {
  return tools.map((t) => {
    const normalized = normalizeToolParameters(
      {
        name: t.function.name,
        description: t.function.description ?? "",
        parameters: (t.function.parameters ?? {}) as Record<string, unknown>,
      } as AnyAgentTool,
      options,
    );
    return {
      name: normalized.name,
      description: normalized.description ?? "",
      parameters: (normalized.parameters ?? {}) as PiTool["parameters"],
    };
  });
}

function convertPiAiResponseToBridge(
  msg: PiAssistantMessage,
  configuredModel: string,
  resolvedProvider: string,
  resolvedModel: string,
  primaryProvider: string,
  fallbackUsed: boolean,
  resolvedProfile?: string,
): BridgeResponse {
  let textContent = "";
  const toolCalls: Array<{ id: string; function: { name: string; arguments: string } }> = [];

  for (const part of msg.content) {
    if (part.type === "text") {
      textContent += part.text;
    } else if (part.type === "toolCall") {
      toolCalls.push({
        id: part.id,
        function: {
          name: part.name,
          arguments: JSON.stringify(part.arguments),
        },
      });
    }
  }

  return {
    ok: true,
    configured_model: configuredModel,
    resolved_model: resolvedModel,
    resolved_provider: resolvedProvider,
    resolved_profile: resolvedProfile,
    fallback_used: fallbackUsed,
    provider_changed: resolvedProvider !== primaryProvider,
    choices: [
      {
        message: {
          content: textContent || null,
          tool_calls: toolCalls.length > 0 ? toolCalls : undefined,
        },
      },
    ],
    usage: {
      prompt_tokens: msg.usage?.input ?? 0,
      completion_tokens: msg.usage?.output ?? 0,
    },
  };
}

// ── Core completion helper ───────────────────────────────────────────────

// Invariant: callKind and promptMode do NOT select different models.
// Driver and reviewer share the same session-resolved model/auth stack.
// callKind is for logging/metrics only; promptMode distinguishes context, not model.

// v1: resolveAndRunOpenClawCompletion() is the temporary authoritative seam.
// There is no existing single OpenClaw helper for this flow today.
// This is a manually assembled path through the model resolution chain.

async function resolveAndRunOpenClawCompletion(
  req: BridgeRequest,
  cfg: OpenClawConfig,
): Promise<BridgeResponse> {
  // v2: session-derived agent resolution. Resolves agentId from session_key
  // (format: "agent:<agentId>:<channel>:..."), then looks up the correct
  // agent directory and MCP server configuration for that agent.
  const { agentId, agentDir } = resolveSessionAgent({
    sessionKey: req.session_key,
    cfg,
  });

  // 1. Resolve primary model (session override or config default)
  const { provider: defaultProvider, model: defaultModel } = resolveConfiguredModelRef({
    cfg,
    defaultProvider: DEFAULT_PROVIDER,
    defaultModel: DEFAULT_MODEL,
  });

  let primaryProvider = defaultProvider;
  let primaryModel = defaultModel;

  // 2. Check session model override (from /model command)
  if (req.session_key) {
    try {
      const storePath = resolveStorePath(cfg.session?.store, { agentId });
      if (fs.existsSync(storePath)) {
        const sessionStore = loadSessionStore(storePath);
        const override = resolveStoredModelOverride({
          sessionStore,
          sessionKey: req.session_key,
        });
        if (override) {
          if (override.provider) primaryProvider = override.provider;
          primaryModel = override.model;
        }
      }
    } catch (err) {
      log.warn(`session model override lookup failed: ${String(err)}`);
    }
  }

  const configuredModel = `${primaryProvider}/${primaryModel}`;

  // 3. Resolve fallback list
  const fallbacksOverride = resolveRunModelFallbacksOverride({
    cfg,
    sessionKey: req.session_key,
  });

  // 4. Run with fallback chain
  const { systemPrompt, messages: piMessages } = convertOpenAIMessagesToPiAi(req.messages);

  const result = await runWithModelFallback({
    cfg,
    provider: primaryProvider,
    model: primaryModel,
    agentDir,
    fallbacksOverride,
    run: async (resolvedProvider, resolvedModel) => {
      const resolved = resolveModel(resolvedProvider, resolvedModel, agentDir, cfg);
      if (resolved.error || !resolved.model) {
        throw new Error(resolved.error ?? `model resolution failed: ${resolvedProvider}/${resolvedModel}`);
      }

      const auth = await getApiKeyForModel({
        model: resolved.model,
        cfg,
        agentDir,
      });

      const context: PiContext = {
        systemPrompt,
        messages: piMessages,
        tools: req.tools
          ? convertOpenAIToolsToPiAi(req.tools, {
              modelProvider: resolvedProvider,
              modelId: resolvedModel,
            })
          : undefined,
      };

      const assistantMsg = await complete(resolved.model, context, {
        apiKey: auth.apiKey,
        maxTokens: req.max_tokens,
        temperature: req.temperature,
      });

      return { assistantMsg, resolvedProvider, resolvedModel, resolvedProfile: auth.profileId };
    },
  });

  const fallbackUsed = result.attempts.length > 0;
  const providerChanged = result.provider !== primaryProvider;
  const modelChanged = result.model !== primaryModel;
  const resolvedProfile = result.result.resolvedProfile;

  const response = convertPiAiResponseToBridge(
    result.result.assistantMsg,
    configuredModel,
    result.provider,
    result.model,
    primaryProvider,
    fallbackUsed,
    resolvedProfile,
  );

  // ── Authoritative model-resolution logging (TS is the source of truth) ──

  const fallbackReason = classifyFallbackReason(result.attempts);
  const configuredFallbacks = fallbacksOverride ?? [];

  // 1. JSONL audit log + atomic summary (same pipeline as agent-runner.ts)
  void appendModelResolutionLog(agentDir, {
    ts: new Date().toISOString(),
    runId: req.request_id,
    kind: "primary",
    sessionKey: req.session_key ?? undefined,
    configuredPrimary: configuredModel,
    configuredFallbacks,
    resolvedModel: result.model,
    resolvedProvider: result.provider,
    resolvedProfile,
    modelChanged,
    providerChanged,
    fallbackReason,
    attempts: result.attempts,
    usage: response.usage
      ? { input: response.usage.prompt_tokens, output: response.usage.completion_tokens }
      : undefined,
  });

  // 2. Subsystem log for operational visibility
  log.info(`resolved`, {
    requestId: req.request_id,
    agentId,
    callKind: req.call_kind,
    configuredModel,
    resolvedModel: result.model,
    resolvedProvider: result.provider,
    resolvedProfile,
    fallbackUsed,
    providerChanged,
  });

  // 3. Fallback alert via Telegram DM (deduped, same pipeline as agent-runner.ts)
  // On fallback, the operator is notified so silent billing changes are visible.
  if (fallbackUsed) {
    const resolvedRef = `${result.provider}/${result.model}`;
    if (shouldSendFallbackAlert(configuredModel, resolvedRef)) {
      const alertMsg = formatModelFallbackAlert({
        configuredPrimary: configuredModel,
        resolvedProvider: result.provider,
        resolvedModel: result.model,
        providerChanged,
        fallbackReason,
      });
      log.warn(alertMsg);
      const operatorChatId = resolveOperatorTelegramChatId(cfg);
      if (operatorChatId) {
        void sendBridgeFallbackAlert({ cfg, alertMsg, operatorChatId });
      }
    }
  }

  return response;
}

// ── Unix socket server ───────────────────────────────────────────────────

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

export function startCompletionsServer(params: {
  cfg: OpenClawConfig;
}): { server: Server; socketPath: string; cleanup: () => void } {
  const uid = os.userInfo().uid;
  const socketPath = `/run/user/${uid}/openclaw-completions.sock`;

  // Clean up stale socket
  try {
    fs.unlinkSync(socketPath);
  } catch {
    // ignore if doesn't exist
  }

  // allowHalfOpen: when the client sends FIN (half-closes writing), the server's
  // writable side stays open so we can send the response after async processing.
  // This matches the Rust client's behavior: shutdown() → read response.
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
          const req: BridgeRequest = JSON.parse(parsed.payload.toString("utf-8"));
          log.info(`bridge request ${req.request_id} call_kind=${req.call_kind} session=${req.session_key ?? "none"}`);

          const response = await resolveAndRunOpenClawCompletion(req, params.cfg);

          const payload = Buffer.from(JSON.stringify(response));
          conn.write(writeLengthPrefixedMessage(payload));
          conn.end();
        } catch (err) {
          const errMsg = err instanceof Error ? err.message : String(err);
          log.error(`bridge request failed: ${errMsg}`);

          try {
            const errResp: BridgeResponse = { ok: false, error: errMsg };
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
      // Client finished sending — try processing if we haven't yet
      tryProcess();
      if (!processing) {
        const errResp: BridgeResponse = { ok: false, error: "incomplete or missing length prefix" };
        const payload = Buffer.from(JSON.stringify(errResp));
        conn.write(writeLengthPrefixedMessage(payload));
        conn.end();
      }
    });

    conn.on("error", (err) => {
      log.warn(`bridge connection error: ${err.message}`);
    });
  });

  server.listen(socketPath, () => {
    try {
      fs.chmodSync(socketPath, 0o600);
    } catch {
      // non-fatal
    }
    log.info(`completions bridge listening on ${socketPath}`);
  });

  server.on("error", (err) => {
    log.error(`completions bridge server error: ${err.message}`);
  });

  const cleanup = () => {
    server.close();
    try {
      fs.unlinkSync(socketPath);
    } catch {
      // ignore
    }
  };

  return { server, socketPath, cleanup };
}
