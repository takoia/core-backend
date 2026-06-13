<script lang="ts">
  import { toasts, history, dismiss, confirmState } from "./toast";

  let bellOpen = $state(false);
  const icon: Record<string, string> = { info: "ℹ️", success: "✅", error: "⛔", warn: "⚠️" };

  function decide(ok: boolean) {
    const c = $confirmState;
    if (c) {
      c.resolve(ok);
      confirmState.set(null);
    }
  }
</script>

<!-- Toasts (top-right) -->
<div class="toasts">
  {#each $toasts as t (t.id)}
    <div class="toast {t.kind}" role="status">
      <span class="ic">{icon[t.kind]}</span>
      <span class="msg">{t.message}</span>
      <button class="x" onclick={() => dismiss(t.id)} aria-label="close">×</button>
    </div>
  {/each}
</div>

<!-- Confirm modal -->
{#if $confirmState}
  <div class="overlay" onclick={() => decide(false)} role="presentation">
    <div class="modal" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
      <p>{$confirmState.message}</p>
      <div class="row">
        <button class="ghost" onclick={() => decide(false)}>Annuler</button>
        <button class="danger" onclick={() => decide(true)}>Confirmer</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .toasts { position: fixed; top: 16px; right: 16px; z-index: 1000; display: flex; flex-direction: column; gap: 0.5rem; }
  .toast { display: flex; align-items: center; gap: 0.5rem; background: var(--panel); border: 1px solid var(--border); border-left-width: 4px; border-radius: 10px; padding: 0.6rem 0.8rem; min-width: 240px; max-width: 380px; box-shadow: 0 10px 30px rgba(0,0,0,0.4); animation: slidein 0.2s ease; }
  .toast.success { border-left-color: var(--ok); }
  .toast.error { border-left-color: var(--err); }
  .toast.warn { border-left-color: var(--warn); }
  .toast.info { border-left-color: var(--accent); }
  .toast .msg { flex: 1; font-size: 0.85rem; }
  .toast .x { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 1.1rem; }
  @keyframes slidein { from { transform: translateX(20px); opacity: 0; } }

  .bell-wrap { position: fixed; bottom: 16px; right: 16px; z-index: 1000; }
  .bell { position: relative; background: var(--panel); border: 1px solid var(--border); border-radius: 50%; width: 44px; height: 44px; cursor: pointer; font-size: 1.1rem; box-shadow: 0 6px 18px rgba(0,0,0,0.35); }
  .count { position: absolute; top: -4px; right: -4px; background: var(--err); color: #fff; border-radius: 20px; font-size: 0.65rem; padding: 0.05rem 0.3rem; }
  .bellpanel { position: absolute; bottom: 54px; right: 0; width: 300px; max-height: 360px; overflow-y: auto; background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 0.7rem; box-shadow: 0 10px 30px rgba(0,0,0,0.45); }
  .bellhead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.4rem; }
  .clear { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 0.78rem; }
  .empty { color: var(--muted); text-align: center; }
  .bitem { display: flex; gap: 0.4rem; align-items: flex-start; padding: 0.35rem 0; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); font-size: 0.8rem; }

  .overlay { position: fixed; inset: 0; background: rgba(0,0,0,0.5); z-index: 1100; display: flex; align-items: center; justify-content: center; }
  .modal { background: var(--panel); border: 1px solid var(--border); border-radius: 14px; padding: 1.5rem; width: 360px; max-width: 90vw; box-shadow: 0 20px 60px rgba(0,0,0,0.5); }
  .modal p { margin: 0 0 1.2rem; }
  .modal .row { display: flex; gap: 0.6rem; justify-content: flex-end; }
  .modal button { border-radius: 8px; padding: 0.5rem 1rem; cursor: pointer; font: inherit; border: 1px solid var(--border); background: var(--bg); color: var(--text); }
  .modal .danger { background: var(--err); border-color: var(--err); color: #2a0707; font-weight: 600; }
</style>
