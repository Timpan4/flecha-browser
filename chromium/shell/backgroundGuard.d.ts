declare namespace chrome.tabs {
  function update(tabId: number, properties: UpdateProperties, backgroundOnly: boolean, deadline?: number): Promise<Tab | undefined>;
  function reload(tabId: number, properties: ReloadProperties, backgroundOnly: boolean, deadline?: number): Promise<void>;
  function goBack(tabId: number, backgroundOnly: boolean, deadline?: number): Promise<void>;
  function goForward(tabId: number, backgroundOnly: boolean, deadline?: number): Promise<void>;
  function remove(tabId: number, backgroundOnly: boolean, deadline?: number, replacementTabId?: number): Promise<void>;
}
