<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api, subscribeJob, type JobSummary } from "./api";
  import { t } from "./i18n";

  const STEPS = ["analyse", "decision", "action", "restitution"];
  const LABELS: Record<string, string> = {
    analyse: "Analyse",
    decision: "Décision",
    action: "Action",
    restitution: "Restitution",
  };

  let jobs: JobSummary[] = [];
  let jobId = "";
  let canvasEl: HTMLCanvasElement;
  let raf = 0;
  let cleanup: (() => void) | null = null;

  let stepStatus: Record<string, string> = {};
  let events: { t: number; msg: string; kind: string }[] = [];
  let pulses: { from: number; born: number }[] = [];

  async function loadJobs() {
    jobs = await api.listJobs();
    if (!jobId && jobs.length) selectJob(jobs[0].id);
  }

  function selectJob(id: string) {
    jobId = id;
    stepStatus = {};
    events = [];
    pulses = [];
    if (cleanup) cleanup();
    cleanup = subscribeJob(id, onEvent);
  }

  function onEvent(ev: Record<string, unknown>) {
    const kind = ev.kind as string;
    const step = ev.step_type as string | undefined;
    const msg = (ev.message as string) ?? "";
    if (kind === "step_started" && step) {
      stepStatus[step] = "running";
      pulses.push({ from: STEPS.indexOf(step), born: performance.now() });
    }
    if (kind === "step_completed" && step) stepStatus[step] = "done";
    events = [{ t: performance.now(), msg, kind }, ...events].slice(0, 12);
    stepStatus = { ...stepStatus };
  }

  // ── Canvas drawing ───────────────────────────────────────────────────────
  function draw() {
    const c = canvasEl;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const rect = c.getBoundingClientRect();
    if (c.width !== rect.width * dpr || c.height !== rect.height * dpr) {
      c.width = rect.width * dpr;
      c.height = rect.height * dpr;
    }
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const W = rect.width;
    const H = rect.height;
    const now = performance.now();

    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#0b0e14";
    ctx.fillRect(0, 0, W, H);

    const cy = H * 0.4;
    const xs = STEPS.map((_, i) => W * (0.14 + i * 0.24));
    const r = Math.min(46, W * 0.05);

    // Connectors
    for (let i = 0; i < xs.length - 1; i++) {
      ctx.strokeStyle = "#232b3d";
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(xs[i] + r, cy);
      ctx.lineTo(xs[i + 1] - r, cy);
      ctx.stroke();
    }

    // Flowing pulses along connectors
    pulses = pulses.filter((p) => now - p.born < 1200 && p.from < xs.length - 1);
    for (const p of pulses) {
      const prog = (now - p.born) / 1200;
      const x = xs[p.from] + r + (xs[p.from + 1] - r - (xs[p.from] + r)) * prog;
      ctx.fillStyle = "#6c8cff";
      ctx.beginPath();
      ctx.arc(x, cy, 5, 0, Math.PI * 2);
      ctx.fill();
    }

    // Nodes
    STEPS.forEach((s, i) => {
      const st = stepStatus[s] ?? "pending";
      const color = st === "done" ? "#36d399" : st === "running" ? "#fbbd23" : "#2b3346";
      if (st === "running") {
        const glow = 0.5 + 0.5 * Math.sin(now / 200);
        ctx.shadowColor = "#fbbd23";
        ctx.shadowBlur = 18 * glow;
      }
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(xs[i], cy, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.shadowBlur = 0;

      ctx.fillStyle = "#0b0e14";
      ctx.font = `${Math.round(r * 0.5)}px sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(i + 1), xs[i], cy);

      ctx.fillStyle = st === "pending" ? "#8a93a6" : "#e6e9f0";
      ctx.font = "14px sans-serif";
      ctx.fillText(LABELS[s], xs[i], cy + r + 22);
    });

    // Event feed
    ctx.textAlign = "left";
    ctx.font = "13px ui-monospace, monospace";
    events.forEach((e, i) => {
      const age = (now - e.t) / 1000;
      const alpha = Math.max(0.25, 1 - i * 0.07);
      ctx.fillStyle =
        e.kind === "approval_required"
          ? `rgba(251,189,35,${alpha})`
          : e.kind === "report"
            ? `rgba(54,211,153,${alpha})`
            : `rgba(138,147,166,${alpha})`;
      ctx.fillText(`• ${e.msg}`.slice(0, 90), 40, H * 0.62 + i * 22);
      void age;
    });

    raf = requestAnimationFrame(draw);
  }

  onMount(() => {
    loadJobs();
    raf = requestAnimationFrame(draw);
  });
  onDestroy(() => {
    cancelAnimationFrame(raf);
    if (cleanup) cleanup();
  });
</script>

<div class="canvas-page">
  <div class="bar">
    <div>
      <strong>{$t("canvas.title")}</strong>
      <span class="muted small"> — {$t("canvas.subtitle")}</span>
    </div>
    <select bind:value={jobId} on:change={() => selectJob(jobId)}>
      {#each jobs as j}
        <option value={j.id}>{(j.title || j.id).slice(0, 40)} · {j.status}</option>
      {/each}
    </select>
  </div>
  {#if !jobId}
    <p class="muted center">{$t("canvas.noJob")}</p>
  {/if}
  <canvas bind:this={canvasEl}></canvas>
</div>

<style>
  .canvas-page { position: fixed; inset: 56px 0 0 0; display: flex; flex-direction: column; background: #0b0e14; }
  .bar { display: flex; justify-content: space-between; align-items: center; padding: 0.7rem 1.5rem; border-bottom: 1px solid var(--border); }
  select { background: #0b0e14; border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.4rem 0.6rem; font: inherit; }
  canvas { flex: 1; width: 100%; }
  .center { text-align: center; margin-top: 2rem; }
  .small { font-size: 0.8rem; }
</style>
