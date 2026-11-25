async function report(result) {
  document.querySelector('#result').textContent = JSON.stringify(result);
  const endpoint = new URL(location.href).searchParams.get('report');
  if (endpoint) await fetch(endpoint, { method: 'POST', body: JSON.stringify(result) });
}

(async () => {
  const simplify = tabs => tabs.map(({ id, active, url }) => ({ id, active, url }));
  const result = {
    id: chrome.runtime.id,
    incognito: chrome.extension.inIncognitoContext,
    storage: await chrome.storage.local.get(),
    reply: await chrome.runtime.sendMessage({ probe: 'popup' }),
    active: simplify(await chrome.tabs.query({ active: true, currentWindow: true })),
    all: simplify(await chrome.tabs.query({})),
    icon: await fetch(chrome.runtime.getURL('icon.png')).then(response => response.ok),
  };
  await report(result);
})().catch(error => report({ error: error.message }));
