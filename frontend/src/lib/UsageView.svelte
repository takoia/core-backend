<script lang="ts">
  import type { UsageTotal } from "./api";
  export let totals: UsageTotal[] = [];
  export let estimatedTotal = 0;
</script>

<div class="card">
  <h2>Usage metering <span class="muted small">— usage-based billing basis</span></h2>
  <p class="muted small">Estimated usage across providers. Real cost is the flat plan; this meters what a
    marketplace consumer would be billed per run.</p>
  <div class="total">≈ ${estimatedTotal.toFixed(4)}</div>
  <table>
    <thead><tr><th>Provider</th><th>Calls</th><th>Prompt tok</th><th>Completion tok</th><th>Est. cost</th></tr></thead>
    <tbody>
      {#each totals as t}
        <tr>
          <td><strong>{t.provider}</strong></td>
          <td>{t.calls}</td>
          <td>{t.prompt_tokens.toLocaleString()}</td>
          <td>{t.completion_tokens.toLocaleString()}</td>
          <td>${t.estimated_cost.toFixed(4)}</td>
        </tr>
      {/each}
      {#if totals.length === 0}
        <tr><td colspan="5" class="muted">No usage yet — run an agent.</td></tr>
      {/if}
    </tbody>
  </table>
</div>

<style>
  .total { font-size: 1.8rem; font-weight: 600; margin: 0.4rem 0 0.8rem; }
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border); }
  td { padding: 0.5rem; border-bottom: 1px solid #161c2a; }
  .small { font-size: 0.78rem; }
</style>
