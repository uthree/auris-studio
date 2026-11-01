// The behaviour of the introduction page: the language and appearance switches, the copy
// buttons, the documentation links and the ambient backdrop. The components are Atelier's.

import { hydrate, setTheme, theme } from "./atelier/atelier.js";
import { AmbientBackdrop } from "./atelier/ambient.js";

const REPO = "https://github.com/uthree/auris-studio";
// Documents that have a Japanese translation beside them as `<name>.ja.md`.
const TRANSLATED = new Set(["features", "composition", "agent-workflows", "installation", "singing-backends"]);

const root = document.documentElement;
const remember = (key, value) => {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A private window may refuse storage; the choice then lasts for this visit only.
  }
};

function setLanguage(lang) {
  root.dataset.lang = lang;
  root.lang = lang;
  for (const button of document.querySelectorAll("#lang > button")) {
    button.setAttribute("aria-pressed", String(button.value === lang));
  }
  for (const link of document.querySelectorAll("[data-doc]")) {
    const name = link.dataset.doc;
    const file = lang === "ja" && TRANSLATED.has(name) ? `${name}.ja.md` : `${name}.md`;
    link.href = `${REPO}/blob/main/docs/${file}`;
  }
}

hydrate();
setLanguage(root.dataset.lang);

document.getElementById("lang").addEventListener("atelier:select", (event) => {
  setLanguage(event.detail.value);
  remember("auris-site-lang", event.detail.value);
});

document.getElementById("appearance").addEventListener("click", () => {
  const appearance = theme().appearance === "dark" ? "light" : "dark";
  setTheme({ appearance });
  remember("auris-site-appearance", appearance);
});

for (const button of document.querySelectorAll(".code__copy")) {
  button.addEventListener("click", async () => {
    const text = button.parentElement.querySelector("code").textContent;
    try {
      await navigator.clipboard.writeText(text);
      button.querySelector("[data-icon]").dataset.icon = "check";
      hydrate(button);
      setTimeout(() => {
        button.querySelector("[data-icon]").dataset.icon = "copy";
        hydrate(button);
      }, 1500);
    } catch {
      // Without clipboard permission the text stays selectable by hand.
    }
  });
}

// The application's own backdrop direction: Glass over a moving scene. Without WebGPU the
// canvas keeps the still gradient atelier.css gives it.
const backdrop = await AmbientBackdrop.create(document.getElementById("ambient"), { scene: "Aurora" });
if (backdrop) {
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");
  const start = () => (reduced.matches ? (backdrop.pause(), backdrop.still()) : backdrop.play());
  start();
  reduced.addEventListener("change", start);
  document.addEventListener("atelier:theme", () => reduced.matches && backdrop.still());
  document.addEventListener("visibilitychange", () => (document.hidden ? backdrop.pause() : start()));
}
