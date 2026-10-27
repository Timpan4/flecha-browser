/**
 * Enter in a text field, but not the Enter that picks an IME candidate (Japanese, Chinese, Korean
 * input). Some IMEs end composition before that keydown, which then reports only keyCode 229.
 */
export const isEnter = (e: { key: string; isComposing: boolean; keyCode?: number }) => e.key === 'Enter' && !e.isComposing && e.keyCode !== 229;
