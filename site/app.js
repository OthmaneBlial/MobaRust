(() => {
  const header = document.querySelector('[data-site-header]');
  const menuToggle = document.querySelector('.menu-toggle');
  const nav = document.querySelector('#site-nav');

  if (menuToggle && nav) {
    menuToggle.addEventListener('click', () => {
      const isOpen = nav.classList.toggle('open');
      menuToggle.setAttribute('aria-expanded', String(isOpen));
      menuToggle.textContent = isOpen ? 'Close' : 'Menu';
    });

    nav.querySelectorAll('a').forEach((link) => {
      link.addEventListener('click', () => {
        nav.classList.remove('open');
        menuToggle.setAttribute('aria-expanded', 'false');
        menuToggle.textContent = 'Menu';
      });
    });
  }

  const previewButtons = document.querySelectorAll('[data-preview-tab]');
  const previewPanels = document.querySelectorAll('[data-preview-panel]');
  const previewLabels = document.querySelectorAll('[data-preview-label]');

  previewButtons.forEach((button) => {
    button.addEventListener('click', () => {
      const tab = button.dataset.previewTab;
      previewButtons.forEach((item) => item.classList.toggle('active', item === button));
      previewLabels.forEach((label) => label.classList.toggle('selected', label.dataset.previewLabel === tab));
      previewPanels.forEach((panel) => {
        panel.hidden = panel.dataset.previewPanel !== tab;
      });
      const crumb = document.querySelector('.crumb');
      if (crumb) crumb.textContent = tab === 'files' ? 'SFTP / staging-files' : tab === 'local' ? 'LOCAL / local-shell' : 'SSH / edge-prod-01';
    });
  });

  document.querySelectorAll('[data-copy]').forEach((button) => {
    button.addEventListener('click', async () => {
      const original = button.textContent;
      const value = button.dataset.copy;
      try {
        await navigator.clipboard.writeText(value);
        button.textContent = 'Copied';
      } catch {
        button.textContent = 'Select manually';
      }
      window.setTimeout(() => { button.textContent = original; }, 1800);
    });
  });

  const year = document.querySelector('[data-year]');
  const demoVideo = document.querySelector('#desktop-demo-video');
  const chapters = [...document.querySelectorAll('[data-demo-time]')];
  if (demoVideo) {
    chapters.forEach((button) => {
      button.addEventListener('click', async () => {
        try {
          if (demoVideo.readyState === 0) {
            await new Promise((resolve, reject) => {
              const cleanup = () => {
                demoVideo.removeEventListener('loadedmetadata', ready);
                demoVideo.removeEventListener('error', failed);
              };
              const ready = () => { cleanup(); resolve(); };
              const failed = () => { cleanup(); reject(new Error('Video unavailable')); };
              demoVideo.addEventListener('loadedmetadata', ready);
              demoVideo.addEventListener('error', failed);
              demoVideo.load();
            });
          }
          demoVideo.currentTime = Number(button.dataset.demoTime);
          await demoVideo.play();
        } catch { demoVideo.focus(); }
      });
    });
    demoVideo.addEventListener('timeupdate', () => {
      const active = chapters.findLast((button) => Number(button.dataset.demoTime) <= demoVideo.currentTime);
      chapters.forEach((button) => button.setAttribute('aria-pressed', String(button === active)));
    });
  }
  if (year) year.textContent = String(new Date().getFullYear());

  if ('IntersectionObserver' in window && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    const observer = new IntersectionObserver((entries, currentObserver) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add('is-visible');
          currentObserver.unobserve(entry.target);
        }
      });
    }, { threshold: 0.12 });
    document.querySelectorAll('.reveal').forEach((element) => observer.observe(element));
  } else {
    document.querySelectorAll('.reveal').forEach((element) => element.classList.add('is-visible'));
  }

  if (header) header.dataset.ready = 'true';
})();
