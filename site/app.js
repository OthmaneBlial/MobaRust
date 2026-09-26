"use strict";

const menuButton = document.querySelector(".menu-toggle");
const siteNav = document.querySelector("#site-nav");

if (menuButton && siteNav) {
  const closeMenu = () => {
    menuButton.setAttribute("aria-expanded", "false");
    siteNav.classList.remove("is-open");
  };

  menuButton.addEventListener("click", () => {
    const isOpen = menuButton.getAttribute("aria-expanded") === "true";
    menuButton.setAttribute("aria-expanded", String(!isOpen));
    siteNav.classList.toggle("is-open", !isOpen);
  });

  siteNav.addEventListener("click", (event) => {
    if (event.target instanceof Element && event.target.closest("a")) closeMenu();
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeMenu();
  });
}

document.querySelectorAll("[data-year]").forEach((node) => {
  node.textContent = String(new Date().getFullYear());
});

const demo = document.querySelector("#desktop-demo");
const chapterButtons = Array.from(document.querySelectorAll("[data-demo-time]"));

if (demo && chapterButtons.length > 0) {
  chapterButtons.forEach((button) => {
    button.addEventListener("click", () => {
      const seek = () => {
        demo.currentTime = Number(button.dataset.demoTime);
      };
      if (demo.readyState >= 1) seek();
      else demo.addEventListener("loadedmetadata", seek, { once: true });
    });
  });

  demo.addEventListener("timeupdate", () => {
    const seconds = demo.currentTime;
    const starts = chapterButtons.map((button) => Number(button.dataset.demoTime));
    const activeIndex = starts.reduce((active, start, index) => seconds >= start ? index : active, -1);
    chapterButtons.forEach((button, index) => {
      if (index === activeIndex) button.setAttribute("aria-current", "true");
      else button.removeAttribute("aria-current");
    });
  });
}

document.querySelectorAll("[data-copy-target]").forEach((button) => {
  button.addEventListener("click", async () => {
    const target = document.getElementById(button.dataset.copyTarget);
    const status = button.parentElement.querySelector(".copy-status");
    if (!target || !status || !navigator.clipboard) {
      if (status) status.textContent = "Clipboard access is unavailable here. Select the commands to copy them.";
      return;
    }

    try {
      await navigator.clipboard.writeText(target.textContent.trim());
      button.textContent = "Copied";
      status.textContent = "Commands copied. Nothing was executed.";
      window.setTimeout(() => {
        button.textContent = "Copy";
      }, 1600);
    } catch {
      status.textContent = "Clipboard access was denied. Select the commands to copy them.";
    }
  });
});
