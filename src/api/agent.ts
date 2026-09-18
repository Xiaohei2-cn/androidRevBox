import { invokeCommand } from "./client";

export type AgentSessionState =
  | "disconnected"
  | "adb_online"
  | "installing"
  | "starting"
  | "handshaking"
  | "ready"
  | "degraded"
  | "incompatible";

export type ProviderHealth = "ready" | "degraded" | "unavailable" | "incompatible" | "faulted";

export interface PermissionInfo {
  shell: boolean;
  root: boolean;
  selinux_enforcing: boolean;
}

export interface ProviderInfo {
  name: string;
  version: string;
  health: ProviderHealth;
  required_permissions?: string[];
  last_error?: string | null;
}

export interface CapabilityInfo {
  method: string;
  version: number;
  provider: string;
  available: boolean;
  unavailable_reason?: string | null;
}

export interface AgentSessionStatus {
  serial: string;
  state: AgentSessionState;
  agentVersion?: string | null;
  protocolVersion?: number | null;
  permissions?: PermissionInfo | null;
  providers: ProviderInfo[];
  capabilities: CapabilityInfo[];
  localPort?: number | null;
  lastError?: string | null;
}

export interface AgentHealth {
  status: "ready" | "degraded";
  agent_version: string;
  protocol_version: number;
  uptime_ms: number;
}

export interface AgentDiagnostics {
  status: AgentSessionStatus;
  health?: AgentHealth | null;
  healthError?: string | null;
  routes: AgentRouteDiagnostics[];
}

export interface AgentRouteDiagnostics {
  serial: string;
  method: string;
  backend: "agent" | "legacy_adb";
  fallbackReason?: string | null;
  agentVersion?: string | null;
  protocolVersion?: number | null;
  recordedAt: number;
}

export const agentApi = {
  status(serial: string): Promise<AgentSessionStatus> {
    return invokeCommand<AgentSessionStatus>("agent_status", { serial });
  },
  statuses(): Promise<AgentSessionStatus[]> {
    return invokeCommand<AgentSessionStatus[]>("agent_statuses");
  },
  install(serial: string): Promise<AgentSessionStatus> {
    return invokeCommand<AgentSessionStatus>("agent_install", { serial });
  },
  restart(serial: string): Promise<AgentSessionStatus> {
    return invokeCommand<AgentSessionStatus>("agent_restart", { serial });
  },
  diagnostics(serial: string): Promise<AgentDiagnostics> {
    return invokeCommand<AgentDiagnostics>("agent_diagnostics", { serial });
  },
};
