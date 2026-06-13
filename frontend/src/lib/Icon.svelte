<script lang="ts">
  // Inline SVG icons (stroke = currentColor) for the menu and UI. Crisp, themed,
  // zero external dependency.
  export let name: string;
  export let size = 18;

  const paths: Record<string, string> = {
    run: "M5 3l14 9-14 9V3z",
    agents: "M12 2a5 5 0 015 5c0 2-1 3-1 5h-8c0-2-1-3-1-5a5 5 0 015-5zM8 17h8M9 21h6",
    canvas: "M3 3h18v14H3zM3 21h18M8 13l3-4 2 3 3-5",
    mcp: "M12 2l8 4v8l-8 4-8-4V6zM12 2v20M4 6l8 4 8-4",
    skills: "M12 2l2.5 6.5L21 9l-5 4 1.5 7L12 16l-5.5 4L8 13 3 9l6.5-.5z",
    settings: "M12 8a4 4 0 100 8 4 4 0 000-8zM3 12h2M19 12h2M12 3v2M12 19v2M5 5l1.5 1.5M17.5 17.5L19 19M19 5l-1.5 1.5M6.5 17.5L5 19",
    usage: "M4 20V10M10 20V4M16 20v-7M22 20H2",
    logs: "M4 5h16M4 10h16M4 15h10M4 20h7",
    builder: "M5 6a2 2 0 100-4 2 2 0 000 4zM19 22a2 2 0 100-4 2 2 0 000 4zM5 6v6a4 4 0 004 4h6M19 18v-2",
    video: "M4 5h16v14H4zM10 9l5 3-5 3V9z",
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={paths[name] ?? paths.run} />
</svg>
