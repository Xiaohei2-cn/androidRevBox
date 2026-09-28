import { invokeCommand } from "./client";
import { listenEvent } from "./events";

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

/**
 * 自动连接探测出来的「这台设备的 Agent 现在缺哪一步」（AR12.5）。
 *
 * 后端算结论、后端选动作：`action` 决定界面上那一个按钮是什么。前端不自己从
 * `decision` 推导，否则网页和后端会对"该不该弹这个按钮"各说一套。
 */
export type AgentProbeDecision =
  | "in_session"
  | "idle_artifact_current"
  | "stale_artifact"
  | "not_installed"
  | "running_elsewhere"
  | "artifact_missing"
  | "unsupported_abi"
  | "device_offline"
  | "probe_failed";

export type AgentAutoAction =
  | "none"
  | "ask_consent"
  | "connecting_allowed"
  | "explicit_takeover"
  | "blocked";

export type AgentAutoOutcome =
  | "already_connected"
  | "connected"
  | "awaited_consent"
  | "deferred_takeover"
  | "skipped"
  | "in_flight"
  | "failed";

export interface AgentProbe {
  serial: string;
  decision: AgentProbeDecision;
  action: AgentAutoAction;
  deviceAbi?: string | null;
  installedSha256?: string | null;
  expectedSha256?: string | null;
  agentRunning: boolean;
  consentGranted: boolean;
  autoEnabled: boolean;
  /** 后端给的"为什么"，中文原文（与其余 Rust 侧用户文案同口径） */
  detail?: string | null;
  probedAt: number;
}

export interface AgentAutoRun {
  serial: string;
  outcome: AgentAutoOutcome;
  probe: AgentProbe;
  error?: string | null;
}

export interface AgentDiagnostics {
  status: AgentSessionStatus;
  health?: AgentHealth | null;
  healthError?: string | null;
  routes: AgentRouteDiagnostics[];
  /** 最近一次自动连接探测；没探过（刚插上/自动连接关着）为 null */
  autoProbe?: AgentProbe | null;
  /** 本次会话里**真的走过** ADB 回退的累计次数（AR12 删除决定的依据） */
  legacyFallbacks: LegacyFallbackTotal[];
}

export interface LegacyFallbackTotal {
  method: string;
  reason: string;
  count: number;
  /** 登记的删除条件，来自 Legacy 能力表 */
  removalStage?: string | null;
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
  /**
   * 主动只读探一次（界面「重新探测」）。永远不写设备：push / chmod / kill /
   * forward 一个都不会发生，所以这个按钮可以放心按。
   */
  probe(serial: string): Promise<AgentProbe> {
    return invokeCommand<AgentProbe>("agent_probe", { serial });
  },
  /** 设备上线自动连接的回执（后端 watch 线程推的） */
  onAutoChanged(handler: (run: AgentAutoRun) => void): Promise<() => void> {
    return listenEvent<AgentAutoRun>("agent://auto", handler);
  },
};
