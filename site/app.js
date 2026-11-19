"use strict";

const menuButton = document.querySelector(".menu-toggle");
const siteNav = document.querySelector("#site-nav");

if (menuButton && siteNav) {
  const setMenu = (open) => {
    menuButton.setAttribute("aria-expanded", String(open));
    siteNav.classList.toggle("is-open", open);
  };
  menuButton.addEventListener("click", () => setMenu(menuButton.getAttribute("aria-expanded") !== "true"));
  siteNav.addEventListener("click", (event) => {
    if (event.target instanceof Element && event.target.closest("a")) setMenu(false);
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") setMenu(false);
  });
}

document.querySelectorAll("[data-year]").forEach((node) => {
  node.textContent = String(new Date().getFullYear());
});

// The yard: sample hosts connect four at a time, like the real group opener.
const yard = document.querySelector("[data-yard]");
if (yard) {
  const boxes = Array.from(yard.querySelectorAll(".box"));
  const count = yard.querySelector("[data-yard-count]");
  const replay = yard.querySelector("[data-yard-run]");
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
  const SLOTS = 4;
  let run = 0;

  const report = () => {
    const done = boxes.filter((box) => box.dataset.state === "on").length;
    count.textContent = `${done} of ${boxes.length} connected`;
  };

  const open = () => {
    const id = ++run;
    boxes.forEach((box) => { box.dataset.state = "off"; });
    report();
    if (reduced.matches) {
      boxes.forEach((box) => { box.dataset.state = "on"; });
      report();
      return;
    }
    let next = 0;
    const start = () => {
      if (id !== run || next >= boxes.length) return;
      const box = boxes[next++];
      box.dataset.state = "wait";
      window.setTimeout(() => {
        if (id !== run) return;
        box.dataset.state = "on";
        report();
        start();
      }, 380 + Math.random() * 520);
    };
    window.setTimeout(() => { for (let i = 0; i < SLOTS; i++) start(); }, 250);
  };

  replay?.addEventListener("click", open);
  if ("IntersectionObserver" in window) {
    const watcher = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        watcher.disconnect();
        open();
      }
    }, { threshold: 0.45 });
    watcher.observe(yard);
  } else {
    open();
  }
}

document.querySelectorAll("[data-copy-target]").forEach((button) => {
  button.addEventListener("click", async () => {
    const target = document.getElementById(button.dataset.copyTarget);
    const status = button.closest(".codebox")?.nextElementSibling;
    const say = (text) => { if (status) status.textContent = text; };
    if (!target || !navigator.clipboard) {
      say("Clipboard access is unavailable here. Select the commands to copy them.");
      return;
    }
    try {
      await navigator.clipboard.writeText(target.textContent.trim());
      button.textContent = "Copied";
      say("Commands copied. Nothing was run.");
      window.setTimeout(() => { button.textContent = "Copy"; }, 1600);
    } catch {
      say("Clipboard access was denied. Select the commands to copy them.");
    }
  });
});
