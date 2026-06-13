<script lang="ts">
  import { history } from "./toast";
  import Icon from "./Icon.svelte";
  let open = $state(false);
  const icon: Record<string, string> = { info: "ℹ️", success: "✅", error: "⛔", warn: "⚠️" };
</script>

<div class="bell-wrap">
  <button class="bell" class:active={open} onclick={() => (open = !open)} aria-label="notifications">
    <Icon name="bell" />
    {#if $history.length}<span class="count">{$history.length}</span>{/if}
  </button>
  {#if open}
    <!-- Fixed overlay so the panel is never clipped by the nav's overflow. -->
    <div class="backdrop" onclick={() => (open = false)} role="presentation"></div>
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
  .bell-wrap { position: relative; flex: 0 0 auto; }
  .bell { display: inline-flex; align-items: center; justify-content: center; position: relative; background: transparent; border: 1px solid transparent; border-radius: 8px; padding: 0.35rem 0.5rem; cursor: pointer; color: var(--muted); }
  .bell:hover, .bell.active { color: var(--text); border-color: var(--border); background: color-mix(in srgb, var(--accent) 18%, transparent); }
  .count { position: absolute; top: -3px; right: -3px; background: var(--err); color: #fff; border-radius: 20px; font-size: 0.62rem; line-height: 1; padding: 0.1rem 0.28rem; }
  .backdrop { position: fixed; inset: 0; z-index: 49; }
  .bellpanel { position: fixed; top: 84px; right: 16px; width: 300px; max-height: 360px; overflow-y: auto; background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 0.7rem; box-shadow: 0 10px 30px rgba(0,0,0,0.45); z-index: 50; }
  .bellhead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.4rem; }
  .clear { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 0.78rem; }
  .empty { color: var(--muted); text-align: center; }
  .bitem { display: flex; gap: 0.4rem; align-items: flex-start; padding: 0.35rem 0; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); font-size: 0.8rem; }
</style>
