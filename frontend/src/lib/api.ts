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

  listAgents: () => req<{ agents: Agent[] }>("/api/agents").then((r) => r.agents),
  getAgent: (id: string) => req<{ agent: Agent; steps: StepConfig[] }>(`/api/agents/${id}`),
  createAgent: (body: Record<string, unknown>) =>
    req<{ id: string }>("/api/agents", { method: "POST", headers: jsonH, body: JSON.stringify(body) }),
  updateSteps: (id: string, steps: unknown[]) =>
    req(`/api/agents/${id}/steps`, { method: "PUT", headers: jsonH, body: JSON.stringify({ steps }) }),
  publishAgent: (id: string, visibility: string, price?: number) =>
    req(`/api/agents/${id}/publish`, {
      method: "POST",
      headers: jsonH,
      body: JSON.stringify({ visibility, price_per_run_usd: price }),
    }),
  importToml: (toml: string) =>
    req<{ id: string }>("/api/agents/import", {
      method: "POST",
      headers: { "Content-Type": "text/plain" },
      body: toml,
    }),
  exportToml: (id: string) => req<string>(`/api/agents/${id}/export`),
  memories: (id: string) =>
    req<{ memories: { key: string; content: string; created_at: string }[] }>(
      `/api/agents/${id}/memories`,
    ).then((r) => r.memories),

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
