/**
 * Shared session_key → agent resolution for VeriCore bridges.
 *
 * Both the completions bridge and the tool broker need to resolve
 * session_key → (agentId, agentDir, mcpServers). This module
 * provides a single source of truth for that resolution.
 */

import type { OpenClawConfig } from "../config/config.js";
import { resolveSessionAgentId, resolveAgentDir, resolveAgentMcpServers } from "../agents/agent-scope.js";
import { parseMcpServers, type McpServerConfig } from "../agents/mcp-common.js";

export type ResolvedSession = {
  agentId: string;
  agentDir: string;
  mcpServers: Record<string, McpServerConfig>;
};

/**
 * Resolve session_key to agentId, agentDir, and MCP server configs.
 *
 * Falls back to default agent when session_key is missing or
 * does not contain an agent prefix.
 */
export function resolveSessionAgent(params: {
  sessionKey?: string | null;
  cfg: OpenClawConfig;
}): ResolvedSession {
  const { sessionKey, cfg } = params;

  const agentId = resolveSessionAgentId({
    sessionKey: sessionKey ?? undefined,
    config: cfg,
  });

  const agentDir = resolveAgentDir(cfg, agentId);
  const rawMcpServers = resolveAgentMcpServers(cfg, agentId);
  const mcpServers = parseMcpServers(rawMcpServers);

  return { agentId, agentDir, mcpServers };
}
