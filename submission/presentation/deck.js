(() => {
  const slides = Array.from(document.querySelectorAll('.slide'));
  const query = new URLSearchParams(window.location.search);
  const renderMode = query.get('render') === '1';
  const currentLabel = document.querySelector('.deck-status .current');
  const totalLabel = document.querySelector('.deck-status .total');
  const progress = document.querySelector('.progress i');
  const autoplayButton = document.querySelector('[data-action="autoplay"]');
  let current = Math.min(Math.max(Number(query.get('slide') || 1) - 1, 0), slides.length - 1);
  let timer = null;
  let autoplay = false;

  if (renderMode) document.body.classList.add('render');
  totalLabel.textContent = String(slides.length).padStart(2, '0');

  const durationFor = (index) => Number(slides[index].dataset.duration || 7) * 1000;

  function show(index, updateUrl = true) {
    current = (index + slides.length) % slides.length;
    slides.forEach((slide, slideIndex) => {
      slide.classList.toggle('active', slideIndex === current);
      slide.setAttribute('aria-hidden', slideIndex === current ? 'false' : 'true');
    });
    currentLabel.textContent = String(current + 1).padStart(2, '0');
    progress.style.width = `${((current + 1) / slides.length) * 100}%`;
    document.title = `${slides[current].dataset.title} · LEZ ⇄ Bitcoin`;

    if (updateUrl && !renderMode) {
      const url = new URL(window.location.href);
      url.searchParams.set('slide', String(current + 1));
      url.searchParams.delete('render');
      history.replaceState(null, '', url);
    }

    if (autoplay) schedule();
  }

  function schedule() {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      if (current === slides.length - 1) {
        setAutoplay(false);
        return;
      }
      show(current + 1);
    }, durationFor(current));
  }

  function setAutoplay(enabled) {
    autoplay = enabled;
    autoplayButton.classList.toggle('playing', autoplay);
    autoplayButton.textContent = autoplay ? 'PAUSE' : 'AUTO';
    window.clearTimeout(timer);
    if (autoplay) schedule();
  }

  function toggleFullscreen() {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen?.();
    } else {
      document.exitFullscreen?.();
    }
  }

  document.querySelector('[data-action="previous"]').addEventListener('click', () => show(current - 1));
  document.querySelector('[data-action="next"]').addEventListener('click', () => show(current + 1));
  autoplayButton.addEventListener('click', () => setAutoplay(!autoplay));
  document.querySelector('[data-action="fullscreen"]').addEventListener('click', toggleFullscreen);

  window.addEventListener('keydown', (event) => {
    if (['ArrowRight', 'PageDown'].includes(event.key)) show(current + 1);
    if (['ArrowLeft', 'PageUp'].includes(event.key)) show(current - 1);
    if (event.key === 'Home') show(0);
    if (event.key === 'End') show(slides.length - 1);
    if (event.key.toLowerCase() === 'f') toggleFullscreen();
    if (event.key === ' ') {
      event.preventDefault();
      setAutoplay(!autoplay);
    }
  });

  let touchStartX = null;
  window.addEventListener('touchstart', (event) => {
    touchStartX = event.changedTouches[0].clientX;
  }, { passive: true });
  window.addEventListener('touchend', (event) => {
    if (touchStartX === null) return;
    const delta = event.changedTouches[0].clientX - touchStartX;
    if (Math.abs(delta) > 50) show(current + (delta < 0 ? 1 : -1));
    touchStartX = null;
  }, { passive: true });

  show(current, false);
  if (query.get('autoplay') === '1' && !renderMode) setAutoplay(true);
  window.__submissionDeckReady = true;
})();
