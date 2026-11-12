(() => {
  "use strict";

  const RELEASE_BASE = "https://github.com/Fazerdino1/4k-free-video-dowloader-/releases/download/v0.2.0";

  const OS_TARGETS = {
    windows: {
      label: "Télécharger pour Windows 64 bits",
      meta: "Windows 10/11 x64 (.zip • 6,0 Mo) — inclut 4k free video dowloader.exe + WebView2Loader.dll",
      url: `${RELEASE_BASE}/4k.free.video.dowloader_0.2.0_windows_x64.zip`,
    },
    ubuntu: {
      label: "Télécharger le paquet .deb (Ubuntu / Debian)",
      meta: "Installateur Debian / Ubuntu / Mint / Pop!_OS / Kali (.deb • 3,9 Mo) • 100 % Open Source",
      url: `${RELEASE_BASE}/4k.free.video.dowloader_0.2.0_amd64.deb`,
    },
    fedora: {
      label: "Télécharger le paquet .rpm (Fedora / RHEL)",
      meta: "Installateur Fedora / Nobara / RHEL / Alma / openSUSE (.rpm • 4,0 Mo) • 100 % Open Source",
      url: `${RELEASE_BASE}/4k.free.video.dowloader-0.2.0-1.x86_64.rpm`,
    },
    linux: {
      label: "Télécharger pour Linux (AppImage)",
      meta: "AppImage universel Linux 64 bits (111 Mo) • Compatible toute distribution",
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
    const floatingBtn = document.getElementById("floating-download-btn");
    if (!heroBtn || !heroBtnLabel || !heroMeta) return;

    const targetKey = detectTargetOS();
    const info = OS_TARGETS[targetKey] || OS_TARGETS.windows;
    heroBtn.href = info.url;
    heroBtnLabel.textContent = info.label;
    heroMeta.textContent = info.meta;

    if (floatingBtn) {
      floatingBtn.href = info.url;
    }
  }

  function initResponsiveNavbar() {
    const header = document.querySelector(".site-header");
    const inner = header?.querySelector(".header-inner");
    const menu = document.getElementById("site-menu");
    if (!header || !inner || !menu) return;

    if (!Object.hasOwn(HTMLElement.prototype, "popover")) {
      import("https://unpkg.com/@oddbird/popover-polyfill@latest/dist/popover.min.js").catch(() => {});
    }

    let lastWidth = header.clientWidth;

    const updateLayout = () => {
      const wasOpen = menu.matches(":popover-open");
      header.classList.add("is-measuring", "is-wide");
      menu.removeAttribute("popover");
      const fits = inner.scrollWidth <= inner.clientWidth;
      header.classList.toggle("is-wide", fits);
      if (!fits) {
        menu.setAttribute("popover", "auto");
        if (wasOpen && typeof menu.showPopover === "function") {
          menu.showPopover();
        }
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

    menu.addEventListener("click", (event) => {
      if (!header.classList.contains("is-wide") && event.target.closest("a")) {
        if (typeof menu.hidePopover === "function") {
          try {
            menu.hidePopover();
          } catch (_) {
            // Ignored if popover isn't open
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

      if (focusTab) {
        nextTab.focus();
      }
      nextTab.scrollIntoView({ block: "nearest", inline: "nearest" });
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

    // Recherche navigateur (Ctrl+F) dans les onglets masqués via hidden="until-found"
    document.addEventListener("beforematch", (event) => {
      const panel = event.target.closest('[role="tabpanel"]');
      if (!panel) return;
      const ownerId = panel.getAttribute("aria-labelledby");
      const ownerTab = tabs.find((t) => t.id === ownerId);
      if (ownerTab) selectTab(ownerTab, false);
    });
  }

  function initFloatingPill() {
    const pill = document.getElementById("floating-pill");
    const heroCta = document.getElementById("hero-cta-sentinel");
    if (!pill || !heroCta || !("IntersectionObserver" in window)) return;

    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          pill.hidden = entry.isIntersecting;
        }
      },
      { threshold: 0.05 }
    );
    observer.observe(heroCta);
  }

  initDownloadDetector();
  initResponsiveNavbar();
  initShowcaseTabs();
  initFloatingPill();
})();
