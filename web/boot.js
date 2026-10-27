// Loads the WebAssembly app. Kept external so the page can run under a strict CSP.
import init from './pkg/apicraft_web.js';

init().catch((err) => {
  const el = document.getElementById('loading');
  if (el) el.textContent = 'Fizl ApiCraft could not start: ' + (err && err.message || err);
  console.error(err);
});
