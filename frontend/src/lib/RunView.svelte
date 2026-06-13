<script lang="ts">
  import { onDestroy } from "svelte";
  import { api, subscribeJob, type Agent, type JobDetail } from "./api";
  import { renderMarkdown } from "./markdown";

  export let agents: Agent[] = [];

  const STEPS = ["analyse", "decision", "action", "restitution"];
  const STEP_LABEL: Record<string, string> = {
    analyse: "Analyse",
    decision: "Decision",
    action: "Action",
    restitution: "Restitution",
  };

  let agentId = "";
  let title = "Weekly AI agents watch";
  let prompt =
    "Monitor recent announcements about AI agents and produce a concise weekly synthesis report.";

  let jobId: string | null = null;
  let jobStatus = "";
  let stepStatus: Record<string, string> = {};
  let logs: string[] = [];
  let report: string | null = null;
  let pendingApprovalId: string | null = null;
  let cleanup: (() => void) | null = null;

  $: if (!agentId && agents.length) agentId = agents[0].id;
  $: selectedAgent = agents.find((a) => a.id === agentId);

  function resetRun() {
    jobStatus = "";
    stepStatus = {};
    logs = [];
    report = null;
    pendingApprovalId = null;
    if (cleanup) cleanup();
    cleanup = null;
  }

  async function launch() {
    if (!agentId) return;
    resetRun();
    const res = await api.createObjective(agentId, title, prompt);
    jobId = res.job_id;
    jobStatus = "queued";
    cleanup = subscribeJob(jobId, handleEvent);
  }

  async function handleEvent(ev: Record<string, unknown>) {
    const kind = ev.kind as string;
    const step = ev.step_type as string | undefined;
    const message = ev.message as string;

    if (kind === "step_started" && step) stepStatus[step] = "running";
    if (kind === "step_completed" && step) stepStatus[step] = "done";
    if (kind === "log") logs = [...logs, message].slice(-40);
    if (kind === "job_status") {
      jobStatus = (ev.status as string) ?? jobStatus;
      logs = [...logs, message].slice(-40);
    }
    if (kind === "approval_required") {
      jobStatus = "awaiting_approval";
      const data = ev.data as { approval_id?: string } | undefined;
      pendingApprovalId = data?.approval_id ?? null;
    }
    if (kind === "report") {
      const data = ev.data as { markdown?: string } | undefined;
      report = data?.markdown ?? null;
    }
    stepStatus = { ...stepStatus };
    // Refresh authoritative detail on terminal-ish transitions.
    if (kind === "report" || jobStatus === "done" || jobStatus === "failed") {
      void refresh();
    }
  }

  async function refresh() {
    if (!jobId) return;
    const d: JobDetail = await api.getJob(jobId);
    report = d.report ?? report;
    jobStatus = d.job.status;
    const pending = d.approvals.find((a) => a.status === "pending");
    pendingApprovalId = pending ? pending.id : null;
  }

  async function decide(decision: "approve" | "reject") {
    if (!pendingApprovalId) return;
    await api.decideApproval(pendingApprovalId, decision);
    pendingApprovalId = null;
    logs = [...logs, `action ${decision}d`];
  }

  onDestroy(() => cleanup && cleanup());
</script>

<div class="grid">
  <div class="card">
    <h2>Launch an objective</h2>
    <label>Agent
      <select bind:value={agentId}>
        {#each agents as a}
          <option value={a.id}>{a.name} · {a.autonomy_level}</option>
        {/each}
      </select>
    </label>
    {#if selectedAgent}
      <p class="muted small">
        {selectedAgent.expertise_domain || "general"} ·
        {selectedAgent.autonomy_level === "full_auto" ? "fully autonomous" : "human-in-the-loop"}
      </p>
    {/if}
    <label>Title <input bind:value={title} /></label>
    <label>Objective
      <textarea rows="4" bind:value={prompt}></textarea>
    </label>
    <button class="primary" on:click={launch} disabled={!agentId}>▶ Run agent</button>
  </div>

  <div class="card">
    <h2>Live run {#if jobStatus}<span class="badge {jobStatus}">{jobStatus}</span>{/if}</h2>
    {#if !jobId}
      <p class="muted">Launch an objective to watch the four steps execute live.</p>
    {:else}
      <div class="timeline">
        {#each STEPS as s}
          <div class="tl-step {stepStatus[s] ?? 'pending'}">
            <span class="tl-dot"></span>
            <span class="tl-label">{STEP_LABEL[s]}</span>
          </div>
        {/each}
      </div>

      {#if pendingApprovalId}
        <div class="approval">
          <strong>Approval required</strong> — the agent wants to act.
          <div class="row">
            <button class="ok" on:click={() => decide("approve")}>✓ Approve</button>
            <button class="danger" on:click={() => decide("reject")}>✗ Reject</button>
          </div>
        </div>
      {/if}

      <div class="logs">
        {#each logs as l}<div class="log">{l}</div>{/each}
      </div>
    {/if}
  </div>
</div>

{#if report}
  <div class="card report">
    <h2>Deliverable</h2>
    <div class="md">{@html renderMarkdown(report)}</div>
  </div>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1.25rem;
  }
  @media (max-width: 800px) {
    .grid { grid-template-columns: 1fr; }
  }
  label {
    display: block;
    margin: 0.75rem 0 0.25rem;
    font-size: 0.85rem;
    color: var(--muted);
  }
  input, textarea, select {
    width: 100%;
    background: #0b0e14;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 8px;
    padding: 0.55rem 0.7rem;
    font: inherit;
  }
  button {
    border: 1px solid var(--border);
    background: #1a2133;
    color: var(--text);
    border-radius: 8px;
    padding: 0.55rem 0.9rem;
    cursor: pointer;
    font: inherit;
  }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; margin-top: 1rem; }
  button.ok { background: var(--ok); border-color: var(--ok); color: #04231a; }
  button.danger { background: var(--err); border-color: var(--err); color: #2a0707; }
  .row { display: flex; gap: 0.6rem; margin-top: 0.6rem; }
  .timeline { display: flex; justify-content: space-between; margin: 0.5rem 0 1rem; }
  .tl-step { display: flex; flex-direction: column; align-items: center; gap: 0.4rem; flex: 1; }
  .tl-dot {
    width: 16px; height: 16px; border-radius: 50%;
    background: var(--border); transition: all 0.3s;
  }
  .tl-step.running .tl-dot { background: var(--warn); box-shadow: 0 0 0 4px #fbbd2333; animation: pulse 1s infinite; }
  .tl-step.done .tl-dot { background: var(--ok); }
  .tl-label { font-size: 0.8rem; color: var(--muted); }
  .tl-step.done .tl-label, .tl-step.running .tl-label { color: var(--text); }
  @keyframes pulse { 50% { opacity: 0.5; } }
  .approval { background: #2a2410; border: 1px solid var(--warn); border-radius: 8px; padding: 0.75rem; margin-bottom: 0.75rem; }
  .logs { max-height: 220px; overflow-y: auto; font-family: ui-monospace, monospace; font-size: 0.78rem; }
  .log { color: var(--muted); padding: 0.15rem 0; border-bottom: 1px solid #161c2a; }
  .badge { font-size: 0.7rem; padding: 0.15rem 0.5rem; border-radius: 20px; background: var(--border); }
  .badge.running, .badge.queued { background: var(--warn); color: #2a2410; }
  .badge.awaiting_approval { background: var(--warn); color: #2a2410; }
  .badge.done { background: var(--ok); color: #04231a; }
  .badge.failed { background: var(--err); color: #2a0707; }
  .small { font-size: 0.8rem; }
  .report :global(h1) { font-size: 1.3rem; }
  .report :global(h2) { font-size: 1.05rem; }
  .md :global(code) { background: #0008; padding: 0.1rem 0.3rem; border-radius: 4px; }
</style>
