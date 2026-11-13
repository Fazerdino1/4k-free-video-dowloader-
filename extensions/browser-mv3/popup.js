document.addEventListener('DOMContentLoaded', async () => {
  const epInput = document.getElementById('endpoint');
  const tokInput = document.getElementById('token');
  const statusEl = document.getElementById('status');

  const saved = await chrome.storage.local.get(['endpoint', 'token']);
  if (saved.endpoint) epInput.value = saved.endpoint;
  if (saved.token) tokInput.value = saved.token;

  document.getElementById('send').addEventListener('click', async () => {
    const endpoint = epInput.value.trim().replace(/\/+$/, '');
    const token = tokInput.value.trim();
    await chrome.storage.local.set({ endpoint, token });

    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab || !tab.url) {
      statusEl.textContent = 'Aucune URL détectée.';
      return;
    }

    const headers = { 'Content-Type': 'application/json' };
    if (token) headers['Authorization'] = `Bearer ${token}`;

    try {
      const resp = await fetch(`${endpoint}/api/enqueue`, {
        method: 'POST',
        headers,
        body: JSON.stringify({ url: tab.url, title: tab.title || tab.url }),
      });
      statusEl.textContent = resp.ok ? 'Ajouté à vidgrab ✓' : `Erreur HTTP ${resp.status}`;
    } catch (e) {
      statusEl.textContent = 'Serveur vidgrab-cli injoignable.';
    }
  });
});
