/** Space colours are pastels; in dark mode they become a deep tint of the same hue. */
export function spaceTint(color: string, dark: boolean) {
  if (!dark) return color;
  const base = [17, 24, 36];
  const rgb = [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
  const mixed = rgb.map((c, i) => Math.round(c * 0.16 + base[i] * 0.84));
  return '#' + mixed.map((c) => c.toString(16).padStart(2, '0')).join('');
}
