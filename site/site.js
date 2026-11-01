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

// The installers carry the version in their names, so a fixed `releases/latest/download/…` URL
// cannot reach them. Ask GitHub which release is the latest and point every download link at
// its installer instead. Until that answers, or if it never does (offline, rate-limited), the
// links lead to the latest release's page, which still has the files on it.
const INSTALLERS = {
  macos: { suffix: "-macos-universal.dmg", en: "Download for macOS", ja: "macOS 版をダウンロード" },
  windows: { suffix: "-windows-x86_64-setup.exe", en: "Download for Windows", ja: "Windows 版をダウンロード" },
};

function visitorPlatform() {
  const platform = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent || "").toLowerCase();
  // iPadOS reports itself as a Mac; a touch screen tells the two apart.
  if (platform.includes("mac") && navigator.maxTouchPoints <= 1) return "macos";
  if (platform.includes("win")) return "windows";
  return null;
}

async function resolveDownloads() {
  // The hero button names the visitor's own system as soon as it is known, even before the
  // release has been looked up; anyone else is sent to the two cards in the install section.
  const own = visitorPlatform();
  const primary = document.getElementById("download-primary");
  if (own) {
    for (const label of primary.querySelectorAll("[data-os-label]")) {
      label.textContent = INSTALLERS[own][label.dataset.osLabel];
    }
  }

  let release;
  try {
    const response = await fetch("https://api.github.com/repos/uthree/auris-studio/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) return;
    release = await response.json();
  } catch {
    return;
  }

  for (const [platform, { suffix }] of Object.entries(INSTALLERS)) {
    const asset = (release.assets || []).find((asset) => asset.name.endsWith(suffix));
    if (!asset) continue;
    for (const link of document.querySelectorAll(`[data-download="${platform}"]`)) {
      link.href = asset.browser_download_url;
    }
    for (const meta of document.querySelectorAll(`[data-download-meta="${platform}"]`)) {
      meta.textContent = `· ${Math.round(asset.size / 1e6)} MB`;
    }
    if (platform === own) primary.href = asset.browser_download_url;
  }
  for (const version of document.querySelectorAll("[data-release-version]")) {
    version.textContent = `· ${release.tag_name}`;
  }
}

resolveDownloads();

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
