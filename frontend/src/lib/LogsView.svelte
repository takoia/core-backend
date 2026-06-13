<script lang="ts">
  import { onMount } from "svelte";
  import { api, type LogEntry } from "./api";
  import { t } from "./i18n";
  import Icon from "./Icon.svelte";

  const PAGE_SIZE = 50;
  const KINDS = [
    "job_status",
    "step_started",
    "step_completed",
    "log",
    "approval_required",
    "report",
  ];

  let logs: LogEntry[] = [];
  let total = 0;
  let q = "";
  let kind = "";
  let jobId = "";
  let offset = 0;
  let loading = false;
  let error = "";

  $: page = Math.floor(offset / PAGE_SIZE) + 1;
  $: pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE));
  $: hasPrev = offset > 0;
  $: hasNext = offset + PAGE_SIZE < total;

  async function load() {
    loading = true;
    error = "";
    try {
      const res = await api.logs({
        q,
        kind,
        job_id: jobId,
        limit: PAGE_SIZE,
        offset,
      });
      logs = res.logs;
      total = res.total;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  // Reset to the first page whenever a filter changes, then reload.
  function applyFilters() {
    offset = 0;
    load();
  }

  function prev() {
    if (!hasPrev) return;
    offset = Math.max(0, offset - PAGE_SIZE);
    load();
  }

  function next() {
    if (!hasNext) return;
    offset += PAGE_SIZE;
    load();
  }

  function fmtTime(ts: string): string {
    const d = new Date(ts);
    return isNaN(d.getTime()) ? ts : d.toLocaleString();
  }

  onMount(load);
</script>

<div class="card">
  <h2>
    <Icon name="logs" />
    {$t("logs.title")}
    <span class="muted small">— {$t("logs.subtitle")}</span>
  </h2>

  <div class="filters">
    <input
      type="text"
      placeholder={$t("logs.search_placeholder")}
      bind:value={q}
      on:keydown={(e) => e.key === "Enter" && applyFilters()}
    />
    <select bind:value={kind} on:change={applyFilters}>
      <option value="">{$t("logs.all_kinds")}</option>
      {#each KINDS as k}
        <option value={k}>{k}</option>
      {/each}
    </select>
    <input
      type="text"
      placeholder={$t("logs.job_id_placeholder")}
      bind:value={jobId}
      on:keydown={(e) => e.key === "Enter" && applyFilters()}
    />
    <button class="primary" on:click={applyFilters} disabled={loading}>
      {$t("logs.apply")}
    </button>
  </div>

  {#if error}
    <p class="err">{error}</p>
  {/if}

  <table>
    <thead>
      <tr>
        <th>{$t("logs.col_time")}</th>
        <th>{$t("logs.col_kind")}</th>
        <th>{$t("logs.col_step")}</th>
        <th>{$t("logs.col_status")}</th>
        <th>{$t("logs.col_message")}</th>
      </tr>
    </thead>
    <tbody>
      {#each logs as l (l.id)}
        <tr>
          <td class="mono nowrap">{fmtTime(l.created_at)}</td>
          <td><span class="badge">{l.kind}</span></td>
          <td class="muted">{l.step_type ?? "—"}</td>
          <td>{l.status ?? "—"}</td>
          <td title={l.job_id}>{l.message}</td>
        </tr>
      {/each}
      {#if logs.length === 0 && !loading}
        <tr><td colspan="5" class="muted">{$t("logs.empty")}</td></tr>
      {/if}
    </tbody>
  </table>

  <div class="pager">
    <button on:click={prev} disabled={!hasPrev || loading}>{$t("logs.prev")}</button>
    <span class="muted small">
      {$t("logs.page")} {page} / {pageCount} · {total} {$t("logs.entries")}
    </span>
    <button on:click={next} disabled={!hasNext || loading}>{$t("logs.next")}</button>
  </div>
</div>

<style>
  h2 { display: flex; align-items: center; gap: 0.5rem; }
  .filters { display: flex; gap: 0.5rem; flex-wrap: wrap; margin: 0.6rem 0 1rem; }
  .filters input, .filters select {
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    padding: 0.4rem 0.6rem;
    font-size: 0.85rem;
  }
  .filters input { min-width: 14rem; }
  button {
    background: var(--panel);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    padding: 0.4rem 0.8rem;
    cursor: pointer;
    font-size: 0.85rem;
  }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button:disabled { opacity: 0.5; cursor: default; }
  table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border); }
  td { padding: 0.5rem; border-bottom: 1px solid var(--border); vertical-align: top; }
  .mono { font-family: ui-monospace, monospace; }
  .nowrap { white-space: nowrap; }
  .badge {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.1rem 0.4rem;
    font-size: 0.75rem;
  }
  .err { color: var(--err); font-size: 0.85rem; }
  .pager { display: flex; align-items: center; gap: 0.8rem; margin-top: 0.9rem; }
  .small { font-size: 0.78rem; }
</style>
