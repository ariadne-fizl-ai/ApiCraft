// OAuth 2 redirect target: hands the query string (code, state) back to the
// app window that opened this popup, on the same origin only.
(function () {
  var msg = document.getElementById('msg');
  var search = window.location.search || ('?' + (window.location.hash || '').replace(/^#/, ''));
  if (window.opener && !window.opener.closed) {
    window.opener.postMessage({ type: 'apicraft-oauth', search: search }, window.location.origin);
    msg.textContent = 'Signed in. You can close this window.';
    setTimeout(function () { window.close(); }, 300);
  } else {
    msg.textContent = 'Sign-in finished, but the app window was closed. Return to Fizl ApiCraft and try again.';
  }
})();
