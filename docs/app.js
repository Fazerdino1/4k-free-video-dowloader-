(() => {
  "use strict";

  const RELEASE_BASE = "https://github.com/Fazerdino1/4k-free-video-dowloader-/releases/download/v0.2.0";

  const OS_TARGETS = {
    windows: {
      label: "Télécharger pour Windows 64 bits (.zip)",
      meta: "Windows 10/11 x64 • Archive 6,0 Mo (4k free video dowloader.exe + WebView2Loader.dll + vidgrab-cli.exe)",
      url: `${RELEASE_BASE}/4k.free.video.dowloader_0.2.0_windows_x64.zip`,
    },
    ubuntu: {
      label: "Télécharger pour Ubuntu / Debian (.deb)",
      meta: "Paquet natif .deb x86_64 (3,9 Mo) • Ubuntu, Debian, Linux Mint, Pop!_OS, Zorin, Kali",
      url: `${RELEASE_BASE}/4k.free.video.dowloader_0.2.0_amd64.deb`,
    },
    fedora: {
      label: "Télécharger pour Fedora / RHEL (.rpm)",
      meta: "Paquet natif .rpm x86_64 (4,0 Mo) • Fedora, Nobara, RHEL, AlmaLinux, Rocky, openSUSE",
      url: `${RELEASE_BASE}/4k.free.video.dowloader-0.2.0-1.x86_64.rpm`,
    },
    linux: {
      label: "Télécharger pour Linux (AppImage)",
      meta: "AppImage universel x86_64 (111 Mo) • Fonctionne sur Arch, CachyOS, Manjaro, SteamOS, Bazzite",
      url: `${RELEASE_BASE}/4k.free.video.dowloader_0.2.0_amd64.AppImage`,
    },
  };

  function detectTargetOS() {
    const platform = (navigator.userAgentData?.platform || navigator.platform || "").toLowerCase();
    const ua = navigator.userAgent.toLowerCase();
    if (platform.includes("win") || ua.includes("windows")) return "windows";
    if (ua.includes("ubuntu") || ua.includes("debian") || ua.includes("mint")) return "ubuntu";
    if (ua.includes("fedora") || ua.includes("red hat") || ua.includes("centos") || ua.includes("suse")) return "fedora";
    if (platform.includes("linux") || ua.includes("linux")) return "linux";
    return "windows";
  }

  function initDownloadDetector() {
    const heroBtn = document.getElementById("hero-download-btn");
    const heroBtnLabel = document.getElementById("hero-download-label");
    const heroMeta = document.getElementById("hero-download-meta");
    if (!heroBtn || !heroBtnLabel || !heroMeta) return;

    const targetKey = detectTargetOS();
    const info = OS_TARGETS[targetKey] || OS_TARGETS.windows;
    heroBtn.href = info.url;
    heroBtnLabel.textContent = info.label;
    heroMeta.textContent = info.meta;
  }

  function initResponsiveNavbar() {
    const header = document.querySelector(".site-header");
    const inner = header?.querySelector(".header-inner");
    const menu = document.getElementById("site-menu");
    const menuBtn = header?.querySelector(".menu-button");
    if (!header || !inner || !menu) return;

    if (!Object.hasOwn(HTMLElement.prototype, "popover")) {
      import("https://unpkg.com/@oddbird/popover-polyfill@latest/dist/popover.min.js").catch(() => {});
    }

    let lastWidth = header.clientWidth;

    const updateLayout = () => {
      const wasOpen = menu.matches(":popover-open, .\\:popover-open");
      header.classList.add("is-measuring", "is-wide");
      menu.removeAttribute("popover");
      const fits = inner.scrollWidth <= inner.clientWidth;
      header.classList.toggle("is-wide", fits);

      if (!fits) {
        menu.setAttribute("popover", "auto");
        if (wasOpen && typeof menu.showPopover === "function") {
          try {
            menu.showPopover();
          } catch (_) {
            // Ignored
          }
        }
      } else if (menuBtn) {
        menuBtn.setAttribute("aria-expanded", "false");
      }

      requestAnimationFrame(() => header.classList.remove("is-measuring"));
    };

    updateLayout();

    const ro = new ResizeObserver(() => {
      const currentWidth = header.clientWidth;
      if (header.classList.contains("is-wide") || currentWidth > lastWidth) {
        updateLayout();
      }
      lastWidth = currentWidth;
    });
    ro.observe(header);
    ro.observe(menu);

    menu.addEventListener("toggle", (event) => {
      if (menuBtn && !header.classList.contains("is-wide")) {
        menuBtn.setAttribute("aria-expanded", String(event.newState === "open"));
      }
    });

    menu.addEventListener("click", (event) => {
      if (!header.classList.contains("is-wide") && event.target.closest("a")) {
        if (typeof menu.hidePopover === "function") {
          try {
            menu.hidePopover();
          } catch (_) {
            // Ignored
          }
        }
      }
    });
  }

  function initShowcaseTabs() {
    const tablist = document.querySelector(".showcase-tablist");
    if (!tablist) return;

    const tabs = Array.from(tablist.querySelectorAll('[role="tab"]'));
    const panels = Array.from(document.querySelectorAll('[role="tabpanel"]'));

    const selectTab = (nextTab, focusTab = false) => {
      for (const tab of tabs) {
        const isSelected = tab === nextTab;
        tab.setAttribute("aria-selected", String(isSelected));
        tab.tabIndex = isSelected ? 0 : -1;
      }

      for (const panel of panels) {
        const matches = panel.getAttribute("aria-labelledby") === nextTab.id;
        if (matches) {
          panel.removeAttribute("hidden");
        } else {
          panel.setAttribute("hidden", "until-found");
        }
      }

      if (focusTab) nextTab.focus();
    };

    tablist.addEventListener("click", (event) => {
      const tab = event.target.closest('[role="tab"]');
      if (tab) selectTab(tab, false);
    });

    tablist.addEventListener("keydown", (event) => {
      const current = event.target.closest('[role="tab"]');
      if (!current) return;

      const idx = tabs.indexOf(current);
      let nextIdx = -1;
      if (event.key === "ArrowRight") nextIdx = (idx + 1) % tabs.length;
      else if (event.key === "ArrowLeft") nextIdx = (idx - 1 + tabs.length) % tabs.length;
      else if (event.key === "Home") nextIdx = 0;
      else if (event.key === "End") nextIdx = tabs.length - 1;

      if (nextIdx !== -1) {
        event.preventDefault();
        selectTab(tabs[nextIdx], true);
      }
    });

    document.addEventListener("beforematch", (event) => {
      const panel = event.target.closest('[role="tabpanel"]');
      if (!panel) return;
      const ownerId = panel.getAttribute("aria-labelledby");
      const ownerTab = tabs.find((t) => t.id === ownerId);
      if (ownerTab) selectTab(ownerTab, false);
    });
  }

  function initCliExplorer() {
    const buttons = Array.from(document.querySelectorAll(".preset-btn"));
    const codeEl = document.getElementById("cli-command-code");
    const copyBtn = document.getElementById("cli-copy-btn");
    if (!buttons.length || !codeEl || !copyBtn) return;

    for (const btn of buttons) {
      btn.addEventListener("click", () => {
        for (const b of buttons) b.setAttribute("aria-pressed", String(b === btn));
        codeEl.textContent = btn.dataset.cmd || "";
        copyBtn.textContent = "Copier";
      });
    }

    copyBtn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(codeEl.textContent || "");
        copyBtn.textContent = "Copié";
        setTimeout(() => {
          copyBtn.textContent = "Copier";
        }, 1600);
      } catch (_) {
        copyBtn.textContent = "Sélectionnez et Ctrl+C";
      }
    });
  }

  initDownloadDetector();
  initResponsiveNavbar();
  initShowcaseTabs();
  initCliExplorer();
})();
