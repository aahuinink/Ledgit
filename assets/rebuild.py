"""Rebuild the Ledgit artwork from the Canva export.

Canva exports the "L" as four stroked segments, each with its own gradient,
and fills the three circles white. Drawn as exported, the joins show: square
ends meeting at an angle, and tone jumping from one gradient to the next.
This script turns each export into what the app ships:

* the four segments become two strokes, each one continuous path with one
  gradient, bending smoothly through the old joins;
* where the two strokes overlap leaving the left circle, both hold one tone
  (TONE_KNUCKLE) for the measured length of the overlap, so the edge of the
  one on top does not show;
* each ring's gradient is aimed at its stroke and holds the stroke's tone
  across the contact, so rings blend into their lines;
* the circles are hollow, with the strokes clipped just inside each ring;
* a `_dark` copy of each file is written with every colour inverted.

Usage, from anywhere in the repo:

    python assets/rebuild.py                 # rebuild from the original export in git
    python assets/rebuild.py --export DIR    # rebuild from a new Canva export in DIR
    python assets/rebuild.py --check         # do the committed files match?

DIR must hold `Icon.svg` and `Ledgit_logo.svg`. After an export, run it and
commit the four files it writes. It always starts from an export, never from
its own output, so running it twice changes nothing.

The tones below are the knobs: percentages from black (0) to white (100) in
the light artwork. The dark copies are their inverse.
"""
import argparse, math, pathlib, re, subprocess

GREY = 45.056534          # the lightest tone in the original gradients, % black->white
TONE_KNUCKLE = 28.0     # where both strokes leave the left circle: between what each would be there
TONE = dict(
    upper_knee=0.0,        # the upper stroke is darkest at its bend...
    upper_end=GREY,        # ...and light where it meets the top-right circle
    lower_mid=GREY,        # the lower stroke is lightest mid-way...
    lower_end=0.0,         # ...and dark where it meets the bottom-right circle
)
RING_FAR = dict(left=0.0, top=0.0, right=GREY)   # tone on the side of each ring away from its strokes

class Unexpected(Exception):
    """The export is not shaped the way this script knows how to rebuild."""

def fmt(v): return f"{v:.3f}".rstrip('0').rstrip('.') or '0'
def world(m, x, y):
    a, b, c, d, e, f = m
    return (a*x + c*y + e, b*x + d*y + f)
def bez(p, t):
    u = 1 - t
    return (u**3*p[0][0] + 3*u*u*t*p[1][0] + 3*u*t*t*p[2][0] + t**3*p[3][0],
            u**3*p[0][1] + 3*u*u*t*p[1][1] + 3*u*t*t*p[2][1] + t**3*p[3][1])
def dist(p, q): return math.hypot(p[0]-q[0], p[1]-q[1])

def smooth_stops(anchors, n=96):
    """Stops for a gradient passing through (offset, tone) anchors, eased with
    smoothstep between them so the tone never changes slope abruptly."""
    anchors = sorted(anchors)
    def tone(t):
        if t <= anchors[0][0]: return anchors[0][1]
        for (t0, v0), (t1, v1) in zip(anchors, anchors[1:]):
            if t <= t1:
                u = 0 if t1 == t0 else (t - t0) / (t1 - t0)
                return v0 + (v1 - v0) * u*u*(3 - 2*u)
        return anchors[-1][1]
    ts = sorted(set([i/n for i in range(n+1)] + [a[0] for a in anchors]))
    return ''.join(f'<stop offset="{fmt(t)}" stop-color="rgb({fmt(tone(t))}%, {fmt(tone(t))}%, {fmt(tone(t))}%)" stop-opacity="1"/>' for t in ts)

def rebuild(s):
    seg_re = re.compile(r'<path stroke-linecap="butt" transform="matrix\(([^)]*)\)"[^>]*? d="([^"]*)" stroke="url\(#(\w+)\)" stroke-width="61"[^>]*/>')
    segs = []
    for m in seg_re.finditer(s):
        mat = [float(v) for v in m.group(1).split(',')]
        nums = [float(v) for v in re.findall(r'-?[\d.]+(?:e-?\d+)?', m.group(2))]
        segs.append(dict(span=m.span(), grad=m.group(3), width=61*math.hypot(mat[0], mat[1]),
                         pts=[world(mat, nums[i], nums[i+1]) for i in range(0, 8, 2)]))
    if len(segs) != 4:
        raise Unexpected(f"expected the L as 4 stroked segments 61 wide, found {len(segs)}")
    w = sum(g['width'] for g in segs) / 4

    # Each segment runs black (P0) to grey (P3): the upper stroke's halves meet
    # black-to-black at its bend, the lower's grey-to-grey.
    pairs = [(i, j) for i in range(4) for j in range(i+1, 4)]
    up = min(pairs, key=lambda p: dist(segs[p[0]]['pts'][0], segs[p[1]]['pts'][0]))
    lo = tuple(k for k in range(4) if k not in up)

    def join(first, second):
        k = ((first[3][0]+second[0][0])/2, (first[3][1]+second[0][1])/2)
        tin = (k[0]-first[2][0], k[1]-first[2][1]); tout = (second[1][0]-k[0], second[1][1]-k[1])
        lin, lout = math.hypot(*tin), math.hypot(*tout)
        u = (tin[0]/lin + tout[0]/lout, tin[1]/lin + tout[1]/lout); n = math.hypot(*u); u = (u[0]/n, u[1]/n)
        return ([first[0], first[1], (k[0]-u[0]*lin, k[1]-u[1]*lin), k],
                [k, (k[0]+u[0]*lout, k[1]+u[1]*lout), second[2], second[3]])

    ring_re = re.compile(r'<path stroke-linecap="butt" transform="matrix\(0\.75, 0, 0, 0\.75, ([\d.]+), ([\d.]+)\)"[^>]*? d="M ([\d.]+) [^"]*" stroke="url\(#(\w+)\)" stroke-width="24"')
    rings = [dict(e=float(m.group(1)), f=float(m.group(2)), grad=m.group(4),
                  c=(float(m.group(1)) + .75*float(m.group(3)), float(m.group(2)) + .75*float(m.group(3))))
             for m in ring_re.finditer(s)]
    if len(rings) != 3:
        raise Unexpected(f"expected 3 ring strokes 24 wide, found {len(rings)}")
    R_OUT = .75 * 82.61; R_IN = R_OUT - .75*12; HOLE = R_IN + 1

    # Orient each stroke to start at the shared (left) circle.
    upper = join(segs[up[0]]['pts'][::-1], segs[up[1]]['pts'])
    lower = join(segs[lo[0]]['pts'], segs[lo[1]]['pts'][::-1])
    near = lambda p: min(rings, key=lambda r: dist(r['c'], p))
    if near(upper[0][0]) is not near(lower[0][0]):
        upper = (upper[1][::-1], upper[0][::-1])
    if near(upper[0][0]) is not near(lower[0][0]):
        lower = (lower[1][::-1], lower[0][::-1])
    left = near(upper[0][0]); top = near(upper[1][3]); right = near(lower[1][3])
    assert len({id(left), id(top), id(right)}) == 3

    def samples(br, n=400):
        """Centre-line points along a two-piece stroke, with unit normals."""
        out = []
        for piece in br:
            for i in range(n):
                t = i / n
                p = bez(piece, t); q = bez(piece, min(t + 1e-3, 1)); r = bez(piece, max(t - 1e-3, 0))
                tx, ty = q[0]-r[0], q[1]-r[1]; l = math.hypot(tx, ty)
                out.append((p, (-ty/l, tx/l)))
        out.append((br[1][3], out[-1][1]))
        return out
    U, L = samples(upper), samples(lower)
    chord = lambda br: (br[0][0], br[1][3])
    def param(br, p):
        a, b = chord(br); v = (b[0]-a[0], b[1]-a[1])
        return ((p[0]-a[0])*v[0] + (p[1]-a[1])*v[1]) / (v[0]**2 + v[1]**2)
    def inside(center_line, p):
        return min(dist(c, p) for c, _ in center_line[::2]) < w/2

    # How far along each stroke the two still overlap outside the left hole:
    # both must hold one tone over that whole stretch, or the edge of the one on
    # top shows.
    def overlap_extent(br, mine, other):
        t = 0.0
        for c, nrm in mine:
            for k in (-1, -.5, 0, .5, 1):
                p = (c[0] + nrm[0]*k*w/2, c[1] + nrm[1]*k*w/2)
                if dist(p, left['c']) > HOLE and inside(other, p):
                    t = max(t, param(br, p))
        return t
    tu = overlap_extent(upper, U, L) + 0.02
    tl = overlap_extent(lower, L, U) + 0.02

    # Where each stroke crosses a ring's outer edge: the contact.
    def contact(center_line, ring, from_end):
        seq = center_line[::-1] if from_end else center_line
        for c, _ in seq:
            if dist(c, ring['c']) > R_OUT:
                return c
    cu_left, cl_left = contact(U, left, False), contact(L, left, False)
    cu_top, cl_right = contact(U, top, True), contact(L, right, True)
    # A stroke holds its end tone from just before it reaches its ring.
    hold = lambda br, p, back: param(br, p) - back

    upper_anchors = [(0, TONE_KNUCKLE), (tu, TONE_KNUCKLE),
                     (param(upper, upper[0][3]), TONE['upper_knee']),
                     (hold(upper, cu_top, 0.05), TONE['upper_end']), (1, TONE['upper_end'])]
    lower_anchors = [(0, TONE_KNUCKLE), (tl, TONE_KNUCKLE),
                     (param(lower, lower[0][3]), TONE['lower_mid']),
                     (hold(lower, cl_right, 0.04), TONE['lower_end']), (1, TONE['lower_end'])]
    for a in (upper_anchors, lower_anchors):
        assert all(x[0] <= y[0] for x, y in zip(a, a[1:])), a

    def linear(gid, a, b, anchors, units='userSpaceOnUse'):
        return (f'<linearGradient id="{gid}" gradientUnits="{units}" x1="{fmt(a[0])}" y1="{fmt(a[1])}" '
                f'x2="{fmt(b[0])}" y2="{fmt(b[1])}">{smooth_stops(anchors)}</linearGradient>')

    defs = linear('l-upper', *chord(upper), upper_anchors) + linear('l-lower', *chord(lower), lower_anchors)

    # Rings: aim the gradient from the far side through the centre to the
    # contact, so the ring wears its stroke's tone where they touch. The left
    # ring aims between its two strokes, which leave it at one tone.
    def unit(p, c):
        v = (p[0]-c[0], p[1]-c[1]); l = math.hypot(*v); return (v[0]/l, v[1]/l)
    aims = {
        id(left): (lambda a, b: unit((a[0]+b[0], a[1]+b[1]), (0, 0)))(unit(cu_left, left['c']), unit(cl_left, left['c'])),
        id(top): unit(cu_top, top['c']),
        id(right): unit(cl_right, right['c']),
    }
    tones = {id(left): (RING_FAR['left'], TONE_KNUCKLE), id(top): (RING_FAR['top'], TONE['upper_end']),
             id(right): (RING_FAR['right'], TONE['lower_end'])}
    for n, r in enumerate(rings):
        u = aims[id(r)]; far, near_tone = tones[id(r)]
        # userSpaceOnUse means the ring path's own space, inside its 0.75 transform.
        to_local = lambda p: ((p[0]-r['e'])/.75, (p[1]-r['f'])/.75)
        a = to_local((r['c'][0]-u[0]*R_OUT, r['c'][1]-u[1]*R_OUT))
        b = to_local((r['c'][0]+u[0]*R_OUT, r['c'][1]+u[1]*R_OUT))
        # Hold the contact tone over the last fifth, which the whole contact arc
        # falls inside (a 46-wide stroke meets a 62-radius ring within ~0.08).
        defs += linear(f'l-ring-{n}', a, b, [(0, far), (0.8, near_tone), (1, near_tone)])
        s = s.replace(f'stroke="url(#{r["grad"]})" stroke-width="24"', f'stroke="url(#l-ring-{n})" stroke-width="24"')
        s = re.sub(r'<linearGradient[^>]*id="%s">.*?</linearGradient>' % r['grad'], '', s, flags=re.S)

    circle = lambda c, rad: (f"M {fmt(c[0]+rad)} {fmt(c[1])} A {fmt(rad)} {fmt(rad)} 0 1 0 {fmt(c[0]-rad)} {fmt(c[1])} "
                             f"A {fmt(rad)} {fmt(rad)} 0 1 0 {fmt(c[0]+rad)} {fmt(c[1])} Z")
    defs = ('<clipPath id="l-outside-rings"><path clip-rule="evenodd" d="M -10 -10 H 778 V 778 H -10 Z '
            + ' '.join(circle(r['c'], HOLE) for r in rings) + '"/></clipPath>') + defs

    def path_d(br):
        p = lambda q: f"{fmt(q[0])} {fmt(q[1])}"
        a, b = br
        return f"M {p(a[0])} C {p(a[1])} {p(a[2])} {p(a[3])} C {p(b[1])} {p(b[2])} {p(b[3])}"
    stroke = lambda br, gid: (f'<path d="{path_d(br)}" fill="none" stroke="url(#{gid})" stroke-width="{fmt(w)}" '
                              f'stroke-linecap="butt" stroke-linejoin="round" clip-path="url(#l-outside-rings)"/>')
    new = stroke(lower, 'l-lower') + stroke(upper, 'l-upper')

    # Earlier edits moved things, so find the old segments afresh.
    spans = [m.span() for m in seg_re.finditer(s)]
    assert len(spans) == 4
    for a, b in sorted(spans, reverse=True):
        s = s[:a] + s[b:]
    s = s[:spans[0][0]] + new + s[spans[0][0]:]
    for g in segs:
        s = re.sub(r'<linearGradient[^>]*id="%s">.*?</linearGradient>' % g['grad'], '', s, flags=re.S)
    s = s.replace('<defs>', '<defs>' + defs, 1)

    fills = 0
    while (i := s.find('<path fill="#ffffff"')) >= 0:
        start, opened = i, 0
        while (m := re.search(r'<g\b[^>]*>$', s[:start])):
            start = m.start(); opened += 1
        end = s.index('/>', i) + 2
        for _ in range(opened):
            assert s.startswith('</g>', end); end += 4
        s = s[:start] + s[end:]; fills += 1
    if fills != 3:
        raise Unexpected(f"expected 3 white circle fills, found {fills}")
    return s, dict(overlap_upper=round(tu, 3), overlap_lower=round(tl, 3),
                   upper=[(round(t, 3), round(v, 1)) for t, v in upper_anchors],
                   lower=[(round(t, 3), round(v, 1)) for t, v in lower_anchors])

def invert(s):
    def inv_rgb(m):
        return 'rgb(' + ', '.join(fmt(100 - float(v.strip().rstrip('%'))) + '%' for v in m.group(1).split(',')) + ')'
    def inv_hex(m):
        h = m.group(1)
        if len(h) == 3: h = ''.join(c*2 for c in h)
        return '#' + ''.join(f"{255-int(h[i:i+2],16):02x}" for i in (0, 2, 4))
    s = re.sub(r'rgb\(([^)]*%[^)]*)\)', inv_rgb, s)
    return re.sub(r'#([0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b', inv_hex, s)

# The commit holding the original Canva exports, before any rebuild.
ORIGINAL = '02043b5'
NAMES = ['Icon', 'Ledgit_logo']

def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--export', metavar='DIR', help='rebuild from a Canva export in DIR instead of git')
    ap.add_argument('--check', action='store_true', help='only report whether the committed files are up to date')
    args = ap.parse_args()

    assets = pathlib.Path(__file__).resolve().parent
    repo = assets.parent

    def original(name):
        if args.export:
            path = pathlib.Path(args.export) / f'{name}.svg'
            if not path.exists():
                raise SystemExit(f'{path}: not found; --export needs a folder holding '
                                 + ' and '.join(f'{n}.svg' for n in NAMES))
            return path.read_text(encoding='utf-8')
        return subprocess.run(['git', '-C', str(repo), 'show', f'{ORIGINAL}:assets/{name}.svg'],
                              capture_output=True, text=True, encoding='utf-8', check=True).stdout

    stale = []
    for name in NAMES:
        try:
            light, info = rebuild(original(name))
        except Unexpected as e:
            raise SystemExit(f'{name}.svg: {e}. Has the design changed shape in Canva?')
        for path, content in [(assets / f'{name}.svg', light), (assets / f'{name}_dark.svg', invert(light))]:
            if args.check:
                if not path.exists() or path.read_text(encoding='utf-8') != content:
                    stale.append(path.name)
            else:
                # newline='' so Windows does not rewrite the line endings.
                path.write_text(content, encoding='utf-8', newline='')
                print(f'wrote {path.relative_to(repo)}')
    if args.check:
        if stale:
            raise SystemExit('out of date: ' + ', '.join(stale) + ' (run python assets/rebuild.py)')
        print('artwork is up to date')

if __name__ == '__main__':
    main()
