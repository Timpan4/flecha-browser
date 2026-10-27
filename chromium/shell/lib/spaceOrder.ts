/**
 * Where a Space dropped on another Space's button goes, in `moveSpace`'s terms: before the Space
 * it was dropped on, or after it when dropped on that button's far half. `undefined` puts it last;
 * `null` means the drop leaves the order as it is.
 */
export function spaceDropBefore(order: string[], dragged: string, over: string, after: boolean): string | undefined | null {
  const at = order.indexOf(over);
  if (dragged === over || at < 0 || !order.includes(dragged)) return null;
  const before = after ? order[at + 1] : over;
  if (before === dragged) return null;
  const rest = order.filter((id) => id !== dragged);
  const i = before === undefined ? rest.length : rest.indexOf(before);
  const next = [...rest.slice(0, i), dragged, ...rest.slice(i)];
  return next.every((id, j) => id === order[j]) ? null : before;
}
