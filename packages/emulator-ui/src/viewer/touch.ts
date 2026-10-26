/** Map the centered, contained LCD image to native touch pixels. */
export function touchPoint(
  bounds: { left: number; top: number; width: number; height: number },
  image: { width: number; height: number },
  input: { width: number; height: number },
  clientX: number,
  clientY: number,
) {
  const scale = Math.min(bounds.width / image.width, bounds.height / image.height);
  const width = image.width * scale,
    height = image.height * scale;
  const x = clientX - bounds.left - (bounds.width - width) / 2;
  const y = clientY - bounds.top - (bounds.height - height) / 2;
  return {
    inside: width > 0 && height > 0 && x >= 0 && x < width && y >= 0 && y < height,
    x: Math.max(0, Math.min(input.width - 1, Math.floor((x * input.width) / width))),
    y: Math.max(0, Math.min(input.height - 1, Math.floor((y * input.height) / height))),
  };
}
