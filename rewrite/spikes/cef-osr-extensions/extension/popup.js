async function inspect() {
  const simplify = tabs => tabs.map(({ id, active, url }) => ({ id, active, url }));

  return {
    id: chrome.runtime.id,
    incognito: chrome.extension.inIncognitoContext,
    storage: await chrome.storage.local.get(),
    reply: await chrome.runtime.sendMessage({ probe: 'popup' }),
    active: simplify(await chrome.tabs.query({ active: true, currentWindow: true })),
    all: simplify(await chrome.tabs.query({})),
    icon: await fetch(chrome.runtime.getURL('icon.png')).then(response => response.ok),
  };
}

async function report(path, body) {
  if (!/^\/popup-report\/[a-z-]+$/.test(path)) throw Error('Invalid popup report path');

  const config = await fetch(chrome.runtime.getURL('collector.json'));
  const { origin } = await config.json();

  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(origin)) throw Error('Invalid collector origin');

  const response = await fetch(origin + path, { method: 'POST', body, redirect: 'error' });

  if (!response.ok) throw Error(`Collector returned HTTP ${response.status}`);
}

async function main() {
  const result = await inspect().catch(error => ({ error: error.message }));
  const body = JSON.stringify(result);
  const output = document.querySelector('#result');
  const path = new URL(location.href).searchParams.get('report');

  output.textContent = body;

  if (path === null) return;

  try {
    await report(path, body);
  } catch (error) {
    output.textContent = `${body}\nCould not report popup result: ${error.message}`;
  }
}

void main();
