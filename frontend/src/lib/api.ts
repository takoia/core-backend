// Thin API client. All calls are same-origin: in dev Vite proxies /api to the
// Rust backend, in production the backend serves this bundle directly.

export interface Health {
  status: string;
  service: string;
  version: string;
}

export interface Agent {
  id: string;
  name: string;
  description: string;
  autonomy_level: string;
  expertise_domain: string;
  visibility: string;
  price_per_run_usd: number;
  runs_count: number;
  created_at: string;
  author?: string;
  icon?: string;
}

export interface StepConfig {
  step_type: string;
  system_prompt: string;
  options: string;
  position: number;
}

export interface JobSummary {
  id: string;
  agent_id: string;
  status: string;
  error: string | null;
  created_at: string;
  title: string | null;
}

export interface Step {
  step_type: string;
  status: string;
  input: string;
  output: string;
  position: number;
  finished_at: string | null;
}

export interface Approval {
  id: string;
  status: string;
  summary: string;
  payload: string;
  created_at: string;
}

export interface JobDetail {
  job: JobSummary;
  steps: Step[];
  approvals: Approval[];
  report: string | null;
}

export interface Connector {
  id: string;
  kind: string;
  name: string;
  base_url: string;
  model: string;
  has_secret: boolean;
  secret_hint: string;
  is_default: boolean;
}

export interface UsageTotal {
  provider: string;
  prompt_tokens: number;
  completion_tokens: number;
  estimated_cost: number;
  calls: number;
}

export interface LogEntry {
  id: string;
  job_id: string;
  kind: string;
  step_type: string | null;
  status: string | null;
  message: string;
  data: string | null;
  created_at: string;
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: { Accept: "application/json", ...(init?.headers ?? {}) },
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`${res.status}: ${text}`);
  }
  const ct = res.headers.get("content-type") ?? "";
  return (ct.includes("json") ? await res.json() : await res.text()) as T;
}

export const api = {
  health: () => req<Health>("/api/health"),

  login: (username: string, password: string) =>
    req<{ token: string; username: string }>("/api/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    }),

  listAgents: () => req<{ agents: Agent[] }>("/api/agents").then((r) => r.agents),
  getAgent: (id: string) => req<{ agent: Agent; steps: StepConfig[] }>(`/api/agents/${id}`),
  createAgent: (body: Record<string, unknown>) =>
    req<{ id: string }>("/api/agents", { method: "POST", headers: jsonH, body: JSON.stringify(body) }),
  updateSteps: (id: string, steps: unknown[]) =>
    req(`/api/agents/${id}/steps`, { method: "PUT", headers: jsonH, body: JSON.stringify({ steps }) }),
  deleteAgent: (id: string) => req(`/api/agents/${id}`, { method: "DELETE" }),
  publishAgent: (id: string, visibility: string, opts?: { pricePerKTokens?: number; revenueShare?: number }) =>
    req(`/api/agents/${id}/publish`, {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({
        visibility,
        price_per_1k_output_tokens: opts?.pricePerKTokens,
        revenue_share: opts?.revenueShare,
      }),
    }),

  // Marketplace: keys, earnings, and the public invoke API.
  scaffold: (description: string) =>
    req<{ persona: string; analyse: string; decision: string; action: string; restitution: string }>(
      "/api/agents/scaffold",
      { method: "POST", headers: jsonH, body: JSON.stringify({ description }) },
    ),

  listKeys: () => req<{ keys: { id: string; name: string; key_prefix: string; revoked: number; last_used_at: string | null }[] }>("/api/keys").then((r) => r.keys),
  createKey: (name: string) => req<{ key: string; prefix: string }>("/api/keys", { method: "POST", headers: jsonH, body: JSON.stringify({ name }) }),
  revokeKey: (id: string) => req(`/api/keys/${id}`, { method: "DELETE" }),
  earnings: () => req<{ invokes: number; output_tokens: number; billed_usd: number; publisher_usd: number }>("/api/marketplace/earnings"),
  invokeAgent: (id: string, key: string, input: string) =>
    req<{ agent: string; output: string; usage: { prompt_tokens: number; completion_tokens: number }; cost_usd: number; publisher_earned_usd: number }>(
      `/api/v1/agents/${id}/invoke`,
      { method: "POST", headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` }, body: JSON.stringify({ input }) },
    ),
  importToml: (toml: string) =>
    req<{ id: string }>("/api/agents/import", {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: toml,
    }),
  exportToml: (id: string) => req<string>(`/api/agents/${id}/export`),
  addAgentMemory: (id: string, content: string, key = "demonstration") =>
    req<{ ok: boolean }>(`/api/agents/${id}/memory`, {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({ content, key }),
    }),
  memories: (id: string) =>
    req<{ memories: { key: string; content: string; created_at: string }[] }>(
      `/api/agents/${id}/memories`,
    ).then((r) => r.memories),

  // ICM memories with native importance metadata (weight, access_count).
  icmMemories: (id: string) =>
    req<{ entries: { summary: string; weight: number; access_count: number; importance: string }[] }>(
      `/api/agents/${id}/icm-memories`,
    ).then((r) => r.entries),

  marketplace: () => req<{ agents: Agent[] }>("/api/marketplace").then((r) => r.agents),

  createObjective: (agent_id: string, title: string, prompt: string) =>
    req<{ job_id: string; objective_id: string }>("/api/objectives", {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({ agent_id, title, prompt }),
    }),

  listJobs: () => req<{ jobs: JobSummary[] }>("/api/jobs").then((r) => r.jobs),
  getJob: (id: string) => req<JobDetail>(`/api/jobs/${id}`),
  decideApproval: (id: string, decision: "approve" | "reject") =>
    req(`/api/approvals/${id}`, { method: "POST", headers: jsonH, body: JSON.stringify({ decision }) }),

  listConnectors: () => req<{ connectors: Connector[] }>("/api/connectors").then((r) => r.connectors),
  upsertConnector: (body: Record<string, unknown>) =>
    req("/api/connectors", { method: "POST", headers: jsonH, body: JSON.stringify(body) }),

  usage: () =>
    req<{ totals: UsageTotal[]; estimated_total_usd: number }>("/api/usage"),

  logs: (params: {
    job_id?: string;
    kind?: string;
    q?: string;
    limit?: number;
    offset?: number;
  } = {}) => {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null && v !== "") qs.set(k, String(v));
    }
    const suffix = qs.toString() ? `?${qs.toString()}` : "";
    return req<{ logs: LogEntry[]; total: number }>(`/api/logs${suffix}`);
  },

  analyzeVideo: (frames: string[], prompt?: string, agentId?: string) =>
    req<{
      items: { info: string; detail: string }[];
      raw: string;
      frame_count: number;
      usage: Record<string, number>;
    }>("/api/video/analyze", {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({ frames, prompt, agent_id: agentId || undefined }),
    }),

  memoryOverview: () =>
    req<{ stats: Record<string, string>; topics: { topic: string; count: number }[] }>(
      "/api/memory/overview",
    ),
  memoryPurge: (topic: string) =>
    req<{ ok: boolean }>(`/api/memory/purge?topic=${encodeURIComponent(topic)}`, {
      method: "POST",
      headers: jsonH,
    }),

  testEmailIntegration: (to: string, subject?: string, body?: string) =>
    req<{ ok: boolean; message: string }>("/api/integrations/email/test", {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({ to, subject, body }),
    }),

  mcpCatalog: () => req<{ servers: McpServer[] }>("/api/mcp/catalog").then((r) => r.servers),
  mcpInstalled: () =>
    req<{ installed: { name: string; connected: boolean }[] }>("/api/mcp/installed").then(
      (r) => r.installed,
    ),
  mcpConnect: (id: string, env: string[] = []) =>
    req<{ ok: boolean; cli_registered: boolean; message: string }>("/api/mcp/connect", {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({ id, env }),
    }),
};

export interface McpServer {
  id: string;
  name: string;
  category: string;
  logo: string;
  description: string;
  transport: string;
  command?: string;
  url?: string;
  docs: string;
}

export interface Skill {
  id: string;
  name: string;
  category: string;
  logo: string;
  description: string;
  repo: string;
  path: string;
  branch: string;
}

export const skillsApi = {
  catalog: () => req<{ skills: Skill[] }>("/api/skills/catalog").then((r) => r.skills),
  installed: () => req<{ installed: string[] }>("/api/skills/installed").then((r) => r.installed),
  install: (s: { id: string; repo: string; path: string; branch: string }) =>
    req<{ ok: boolean; source: string }>("/api/skills/install", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(s),
    }),
  github: (repo: string, path = "") =>
    req<{ folders: { name: string; path: string }[] }>(
      `/api/skills/github?repo=${encodeURIComponent(repo)}&path=${encodeURIComponent(path)}`,
    ).then((r) => r.folders),
};

const jsonH = { "Content-Type": "application/json" };

// Subscribe to a job's live SSE event stream. Returns a cleanup function.
export function subscribeJob(
  jobId: string,
  onEvent: (ev: Record<string, unknown>) => void,
): () => void {
  const es = new EventSource(`/api/jobs/${jobId}/events`);
  es.addEventListener("progress", (e) => {
    try {
      onEvent(JSON.parse((e as MessageEvent).data));
    } catch {
      /* ignore malformed */
    }
  });
  return () => es.close();
}
