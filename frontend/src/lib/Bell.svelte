<script lang="ts">
  import { history } from "./toast";
  let open = $state(false);
  const icon: Record<string, string> = { info: "ℹ️", success: "✅", error: "⛔", warn: "⚠️" };
</script>

<div class="bell-wrap">
  <button class="bell" onclick={() => (open = !open)} aria-label="notifications">
    🔔{#if $history.length}<span class="count">{$history.length}</span>{/if}
  </button>
  {#if open}
    <div class="bellpanel">
      <div class="bellhead"><strong>Notifications</strong><button class="clear" onclick={() => history.set([])}>clear</button></div>
      {#if $history.length === 0}<p class="empty">—</p>{/if}
      {#each $history as t}
        <div class="bitem"><span>{icon[t.kind]}</span><span>{t.message}</span></div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .bell-wrap { position: relative; }
  .bell { position: relative; background: transparent; border: 1px solid var(--border); border-radius: 8px; padding: 0.35rem 0.55rem; cursor: pointer; font-size: 1rem; color: var(--text); }
  .count { position: absolute; top: -5px; right: -5px; background: var(--err); color: #fff; border-radius: 20px; font-size: 0.62rem; padding: 0.02rem 0.28rem; }
  .bellpanel { position: absolute; top: 42px; right: 0; width: 300px; max-height: 360px; overflow-y: auto; background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 0.7rem; box-shadow: 0 10px 30px rgba(0,0,0,0.45); z-index: 50; }
  .bellhead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.4rem; }
  .clear { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 0.78rem; }
  .empty { color: var(--muted); text-align: center; }
  .bitem { display: flex; gap: 0.4rem; align-items: flex-start; padding: 0.35rem 0; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); font-size: 0.8rem; }
</style>
