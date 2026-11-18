declare namespace chrome.tabs {
  function update(tabId: number, properties: UpdateProperties, backgroundOnly: boolean): Promise<Tab | undefined>;
  function reload(tabId: number, properties: ReloadProperties, backgroundOnly: boolean): Promise<void>;
  function goBack(tabId: number, backgroundOnly: boolean): Promise<void>;
  function goForward(tabId: number, backgroundOnly: boolean): Promise<void>;
  function remove(tabId: number, backgroundOnly: boolean): Promise<void>;
}
