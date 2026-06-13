// Theme system: each theme overrides the CSS custom properties used across the
// app. The "Ocean" theme matches the TakoIA octopus-and-wave branding.

import { writable } from "svelte/store";

export interface Theme {
  id: string;
  name: string;
  vars: Record<string, string>;
}

export const THEMES: Theme[] = [
  {
    id: "ocean",
    name: "Ocean",
    vars: {
      "--bg": "#07141f",
      "--bg-glow": "#0e3346",
      "--panel": "#0c2230",
      "--border": "#17414f",
      "--text": "#dff3f7",
      "--muted": "#7fa3b0",
      "--accent": "#2fc4d6",
      "--ok": "#36d399",
      "--warn": "#fbbd23",
      "--err": "#f87272",
    },
  },
  {
    id: "midnight",
    name: "Midnight",
    vars: {
      "--bg": "#0b0e14",
      "--bg-glow": "#161c2a",
      "--panel": "#131826",
      "--border": "#232b3d",
      "--text": "#e6e9f0",
      "--muted": "#8a93a6",
      "--accent": "#6c8cff",
      "--ok": "#36d399",
      "--warn": "#fbbd23",
      "--err": "#f87272",
    },
  },
  {
    id: "abyss",
    name: "Abyss",
    vars: {
      "--bg": "#04100e",
      "--bg-glow": "#0a2e2a",
      "--panel": "#0a201d",
      "--border": "#16433c",
      "--text": "#dcf5ee",
      "--muted": "#7fa99f",
      "--accent": "#15d6a8",
      "--ok": "#36d399",
      "--warn": "#fbbd23",
      "--err": "#f87272",
    },
  },
  {
    id: "coral",
    name: "Coral",
    vars: {
      "--bg": "#160d14",
      "--bg-glow": "#3a1626",
      "--panel": "#241420",
      "--border": "#4a2336",
      "--text": "#f7e9f0",
      "--muted": "#b08a9d",
      "--accent": "#ff5e8a",
      "--ok": "#36d399",
      "--warn": "#fbbd23",
      "--err": "#f87272",
    },
  },
];

function applyTheme(id: string) {
  const theme = THEMES.find((t) => t.id === id) ?? THEMES[0];
  const root = document.documentElement;
  for (const [k, v] of Object.entries(theme.vars)) {
    root.style.setProperty(k, v);
  }
}

const stored = localStorage.getItem("theme") ?? "ocean";
export const themeId = writable<string>(stored);
themeId.subscribe((id) => {
  localStorage.setItem("theme", id);
  applyTheme(id);
});

export function setTheme(id: string) {
  themeId.set(id);
}
