<script lang="ts">
  import { api } from "./api";
  import { t } from "./i18n";

  const MAX_FRAMES = 40;
  const FRAME_WIDTH = 640;

  let videoEl: HTMLVideoElement;
  let videoUrl: string | null = null;
  let prompt = "";
  let status = "";
  let busy = false;

  // Extracted info items + per-item human confirmation (null = undecided).
  type Item = { info: string; detail: string; ok: boolean | null };
  let items: Item[] = [];

  // Screen recording state.
  let recorder: MediaRecorder | null = null;
  let recording = false;
  let chunks: BlobPart[] = [];

  function setVideo(blob: Blob) {
    if (videoUrl) URL.revokeObjectURL(videoUrl);
    videoUrl = URL.createObjectURL(blob);
    items = [];
  }

  function onFile(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (file) setVideo(file);
  }

  async function startRecording() {
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
      chunks = [];
      recorder = new MediaRecorder(stream);
      recorder.ondataavailable = (ev) => ev.data.size && chunks.push(ev.data);
      recorder.onstop = () => {
        stream.getTracks().forEach((tr) => tr.stop());
        setVideo(new Blob(chunks, { type: "video/webm" }));
      };
      recorder.start();
      recording = true;
      status = $t("video.recording");
    } catch (e) {
      status = e instanceof Error ? e.message : String(e);
    }
  }

  function stopRecording() {
    recorder?.stop();
    recording = false;
    status = "";
  }

  function seekTo(time: number): Promise<void> {
    return new Promise((resolve) => {
      const onSeeked = () => {
        videoEl.removeEventListener("seeked", onSeeked);
        resolve();
      };
      videoEl.addEventListener("seeked", onSeeked);
      videoEl.currentTime = time;
    });
  }

  async function extractFrames(): Promise<string[]> {
    const duration = videoEl.duration;
    const scale = Math.min(1, FRAME_WIDTH / (videoEl.videoWidth || FRAME_WIDTH));
    const canvas = document.createElement("canvas");
    canvas.width = Math.round((videoEl.videoWidth || FRAME_WIDTH) * scale);
    canvas.height = Math.round((videoEl.videoHeight || 360) * scale);
    const ctx = canvas.getContext("2d")!;
    const count = Math.min(MAX_FRAMES, Math.max(1, Math.floor(duration)));
    const out: string[] = [];
    for (let s = 0; s < count; s++) {
      status = $t("video.extracting", { i: s + 1, n: count });
      await seekTo(Math.min(s + 0.05, duration - 0.05));
      ctx.drawImage(videoEl, 0, 0, canvas.width, canvas.height);
      out.push(canvas.toDataURL("image/png"));
    }
    return out;
  }

  async function analyze() {
    if (!videoUrl) return;
    busy = true;
    items = [];
    try {
      const frames = await extractFrames();
      status = $t("video.analyzing", { n: frames.length });
      const r = await api.analyzeVideo(frames, prompt || undefined);
      items = r.items.map((it) => ({ ...it, ok: null }));
      status = $t("video.done", { n: r.frame_count });
    } catch (e) {
      status = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  $: confirmed = items.filter((i) => i.ok === true);
</script>

<div class="card">
  <h2>{$t("video.title")} <span class="muted small">— {$t("video.subtitle")}</span></h2>
  <p class="muted small">{$t("video.hint")}</p>

  <div class="sources">
    {#if !recording}
      <button class="rec" on:click={startRecording}>● {$t("video.record")}</button>
    {:else}
      <button class="stop" on:click={stopRecording}>■ {$t("video.stop")}</button>
    {/if}
    <span class="muted small">{$t("video.or")}</span>
    <input type="file" accept="video/*" on:change={onFile} />
  </div>

  {#if videoUrl}
    <video bind:this={videoEl} src={videoUrl} controls preload="auto" muted></video>
    <label>{$t("video.prompt")}
      <textarea rows="2" bind:value={prompt} placeholder={$t("video.promptPlaceholder")}></textarea>
    </label>
    <div class="row">
      <button class="primary" on:click={analyze} disabled={busy}>{busy ? "…" : $t("video.analyze")}</button>
      {#if status}<span class="muted small">{status}</span>{/if}
    </div>
  {/if}
</div>

{#if items.length}
  <div class="card">
    <h2>{$t("video.extracted")} <span class="muted small">— {$t("video.confirmHint")}</span></h2>
    {#each items as it}
      <div class="item" class:ok={it.ok === true} class:no={it.ok === false}>
        <div class="info">
          <strong>{it.info}</strong>
          <span class="muted small">{it.detail}</span>
        </div>
        <div class="decide">
          <button class="yes" class:on={it.ok === true} on:click={() => (it.ok = true)}>✓</button>
          <button class="nope" class:on={it.ok === false} on:click={() => (it.ok = false)}>✗</button>
        </div>
      </div>
    {/each}
    <p class="muted small summary">{$t("video.confirmedCount", { n: confirmed.length, total: items.length })}</p>
  </div>
{/if}

<style>
  .sources { display: flex; align-items: center; gap: 0.8rem; flex-wrap: wrap; margin: 0.8rem 0; }
  input[type="file"] { color: var(--muted); }
  .rec, .stop { border: 1px solid var(--err); background: color-mix(in srgb, var(--err) 18%, transparent); color: var(--err); border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; font-weight: 600; }
  .stop { background: var(--err); color: #2a0707; }
  video { width: 100%; max-height: 380px; border-radius: 10px; border: 1px solid var(--border); background: #000; margin-top: 0.3rem; }
  label { display: block; font-size: 0.85rem; color: var(--muted); margin-top: 0.8rem; }
  textarea { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.55rem 0.7rem; font: inherit; margin-top: 0.25rem; }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.8rem; }
  button.primary { background: var(--accent); border: 1px solid var(--accent); color: #04231a; font-weight: 600; border-radius: 8px; padding: 0.55rem 1rem; cursor: pointer; font: inherit; }
  button.primary:disabled { opacity: 0.6; cursor: default; }
  .item { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding: 0.55rem 0.6rem; border: 1px solid var(--border); border-radius: 8px; margin-top: 0.5rem; }
  .item.ok { border-color: var(--ok); background: color-mix(in srgb, var(--ok) 10%, transparent); }
  .item.no { border-color: var(--err); opacity: 0.6; }
  .info { display: flex; flex-direction: column; min-width: 0; }
  .decide { display: flex; gap: 0.3rem; flex: 0 0 auto; }
  .decide button { width: 30px; height: 30px; border-radius: 7px; border: 1px solid var(--border); background: var(--bg); color: var(--muted); cursor: pointer; font-size: 0.9rem; }
  .decide .yes.on { background: var(--ok); color: #04231a; border-color: var(--ok); }
  .decide .nope.on { background: var(--err); color: #2a0707; border-color: var(--err); }
  .summary { margin-top: 0.8rem; }
  .small { font-size: 0.78rem; }
</style>
