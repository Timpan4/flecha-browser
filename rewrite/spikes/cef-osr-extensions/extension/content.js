document.documentElement.dataset.flechaInjected = String(document.readyState);

(async () => {
  await chrome.storage.local.set({ content: location.pathname });
  const reply = await chrome.runtime.sendMessage({ probe: location.pathname });

  if (!reply || Object.hasOwn(reply, 'error')) throw Error(reply?.error ?? 'Missing worker reply');
  document.documentElement.dataset.flechaReply = JSON.stringify(reply);
})().catch(error => {
  document.documentElement.dataset.flechaError = error.message;
});
