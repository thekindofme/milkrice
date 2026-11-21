/* Milkrice — site. Small jobs: copy buttons, campaign tags on Download links, and two fallbacks for browsers
   without CSS scroll-driven animations (the story's progress, and section reveals). */
(() => {
  'use strict';
  const root = document.documentElement;

  document.querySelectorAll('[data-copy]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const text = document.getElementById(btn.dataset.copy).textContent.trim();
      try { await navigator.clipboard.writeText(text); } catch { return; }
      btn.textContent = 'Copied'; btn.dataset.done = '';
      setTimeout(() => { btn.textContent = 'Copy'; delete btn.dataset.done; }, 1600);
    });
  });

  // Download links: carry this page's campaign tags (?ref=blog, utm_*) on to /download,
  // so a click can be credited to the link that brought the visitor here. Nothing is stored.
  const tags = new URLSearchParams();
  new URLSearchParams(location.search).forEach((v, k) => {
    if (k === 'ref' || k.startsWith('utm_')) tags.set(k, v);
  });
  if ([...tags].length) {
    document.querySelectorAll('a[href^="/download"]').forEach((a) => {
      const u = new URL(a.href);
      tags.forEach((v, k) => u.searchParams.set(k, v));
      a.href = u.pathname + u.search;
    });
  }

  // The film section stays out of the page until its poster is actually there.
  const film = document.getElementById('film');
  if (film) {
    const probe = new Image();
    probe.onload = () => { film.hidden = false; };
    probe.src = film.querySelector('video').getAttribute('poster');
  }

  if (!root.classList.contains('no-sda')) return;

  // Story: map scroll position through the pinned section to --p (0..1); CSS does the rest.
  const story = document.querySelector('.story');
  if (story) {
    let queued = false;
    const update = () => {
      queued = false;
      const r = story.getBoundingClientRect();
      const span = r.height - innerHeight;
      const p = span > 0 ? Math.min(1, Math.max(0, -r.top / span)) : 1;
      story.style.setProperty('--p', p.toFixed(4));
    };
    const ask = () => { if (!queued) { queued = true; requestAnimationFrame(update); } };
    addEventListener('scroll', ask, { passive: true });
    addEventListener('resize', ask);
    update();
  }

  // Reveals.
  const items = document.querySelectorAll('.rv');
  if (!('IntersectionObserver' in window)) { items.forEach((el) => el.classList.add('in')); return; }
  const io = new IntersectionObserver((entries) => {
    entries.forEach((e) => { if (e.isIntersecting) { e.target.classList.add('in'); io.unobserve(e.target); } });
  }, { rootMargin: '0px 0px -8% 0px' });
  items.forEach((el) => io.observe(el));
})();
