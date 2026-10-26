"""Packs one rendered view (x70_layers.py output) into a compact art file for the
setup wizard's Lottie scenes:

    python3 src/setup/art/pack.py <layers-dir> <out.json>

Fill images are cropped to their content, reduced to a small palette and embedded as
base64 PNG (placed at half scale: they are rendered at 2x). Line art is simplified
(Ramer-Douglas-Peucker, 0.45 px) and grouped by stroke width, as Lottie shape groups.
Needs Pillow.
"""
import base64, io, json, re, sys
from pathlib import Path
from PIL import Image

EPS = 0.45


def simplify(pts, eps=EPS):
    if len(pts) < 3:
        return pts
    (x0, y0), (x1, y1) = pts[0], pts[-1]
    dx, dy = x1 - x0, y1 - y0
    norm = (dx * dx + dy * dy) ** 0.5 or 1e-9
    worst, idx = 0.0, 0
    for i in range(1, len(pts) - 1):
        x, y = pts[i]
        dist = abs(dy * x - dx * y + x1 * y0 - y1 * x0) / norm
        if dist > worst:
            worst, idx = dist, i
    if worst <= eps:
        return [pts[0], pts[-1]]
    return simplify(pts[: idx + 1], eps)[:-1] + simplify(pts[idx:], eps)


def lines(svg_path):
    """Stroke groups [{width, paths: [[[x, y], ...], ...]}] from a Freestyle SVG."""
    text = Path(svg_path).read_text()
    groups = {}
    for m in re.finditer(r'<path[^>]*stroke-width="([\d.]+)"[^>]*d="([^"]+)"', text):
        width = round(float(m.group(1)), 1)
        for run in re.split(r'\s*M\s*', m.group(2).strip()):
            nums = [float(v) for v in re.findall(r'-?\d+\.?\d*', run)]
            pts = [[nums[i], nums[i + 1]] for i in range(0, len(nums) - 1, 2)]
            if len(pts) < 2:
                continue
            pts = [[round(x, 1), round(y, 1)] for x, y in simplify(pts)]
            groups.setdefault(width, []).append(pts)
    return [{'width': w, 'paths': p} for w, p in sorted(groups.items(), reverse=True)]


def image(png_path, colours=40):
    """Crop to content, quantize, encode: {x, y, w, h} in 1x pixels plus base64 PNG data."""
    im = Image.open(png_path).convert('RGBA')
    box = im.getchannel('A').getbbox()
    if not box:
        return None
    im = im.crop(box)
    q = im.quantize(colors=colours, method=Image.Quantize.FASTOCTREE, dither=Image.Dither.NONE)
    buf = io.BytesIO()
    q.save(buf, 'PNG', optimize=True)
    x0, y0, x1, y1 = box
    return {'x': x0 / 2, 'y': y0 / 2, 'w': (x1 - x0) / 2, 'h': (y1 - y0) / 2, 'px': [x1 - x0, y1 - y0],
            'data': 'data:image/png;base64,' + base64.b64encode(buf.getvalue()).decode()}


def main(src, out):
    src = Path(src)
    anchors = json.loads((src / 'anchors.json').read_text())
    levers = sorted(src.glob('lever-*.svg'), key=lambda p: int(p.stem.split('-')[1]))
    art = {
        'anchors': anchors,
        'body': {'image': image(src / 'fill.png'), 'lines': lines(src / 'lines.svg')},
        'lever': [{'image': image(src / f'{p.stem}-fill.png'), 'lines': lines(p)} for p in levers],
    }
    Path(out).write_text(json.dumps(art, separators=(',', ':')))
    n = sum(len(g['paths']) for g in art['body']['lines'])
    print(f'{out}: {Path(out).stat().st_size / 1024:.0f} KB, {n} body strokes, {len(levers)} lever steps')


if __name__ == '__main__':
    main(*sys.argv[1:3])
