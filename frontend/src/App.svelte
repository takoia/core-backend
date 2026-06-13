<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Health } from "./lib/api";

  let health: Health | null = null;
  let error: string | null = null;

  async function refresh() {
    error = null;
    try {
      health = await api.health();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      health = null;
    }
  }

  onMount(refresh);
</script>

<div class="container">
  <div class="brand">
    <h1>TakoIA</h1>
    <span class="tag">autonomous agents · analyse → decision → action → restitution</span>
  </div>

  <div class="card">
    <div class="status-line">
      <span class="dot" class:ok={!!health} class:err={!!error}></span>
      {#if health}
        <span>Backend <strong>{health.service}</strong> v{health.version} — {health.status}</span>
      {:else if error}
        <span>Backend unreachable — <span class="muted">{error}</span></span>
      {:else}
        <span class="muted">Checking backend…</span>
      {/if}
    </div>
    <p class="muted" style="margin-bottom:0">
      P0 foundations online. Next: agents, objectives, and the live 4-step run dashboard.
    </p>
  </div>
</div>
