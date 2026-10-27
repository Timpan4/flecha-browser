/** Enter in a text field, but not the Enter that picks an IME candidate (Japanese, Chinese, Korean input). */
export const isEnter = (e: { key: string; isComposing: boolean }) => e.key === 'Enter' && !e.isComposing;
