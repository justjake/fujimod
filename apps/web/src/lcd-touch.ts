export function lcdTouchPosition(
  clientX: number,
  clientY: number,
  bounds: Pick<DOMRect, 'left' | 'top' | 'width' | 'height'>,
  width: number,
  height: number,
) {
  return {
    x: Math.max(
      0,
      Math.min(width - 1, Math.round(((clientX - bounds.left) * width) / bounds.width)),
    ),
    y: Math.max(
      0,
      Math.min(height - 1, Math.round(((clientY - bounds.top) * height) / bounds.height)),
    ),
  };
}
