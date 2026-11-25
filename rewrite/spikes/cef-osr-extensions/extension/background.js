chrome.runtime.onInstalled.addListener(() => {
  chrome.storage.local.set({ installed: true });
});

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  chrome.storage.local.get('content').then(
    ({ content }) => respond({
      echoed: message.probe,
      stored: content ?? null,
      tabId: sender.tab?.id ?? null,
      incognito: sender.tab?.incognito ?? false,
    }),
    error => respond({ error: error.message }),
  );

  return true;
});
