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
