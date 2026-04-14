/**
 * Render a 3D-perspective donut chart into an <svg> element.
 *
 * @param {SVGElement} svg   Target SVG element. Its viewBox is set automatically.
 * @param {Array}      data  [{ label, value, color }, ...]  (values in any unit — normalized internally)
 * @param {Object}     opts  Optional overrides (see source for list)
 */
export function renderDonut(svg, data, opts = {}) {
  const rOuter      = opts.rOuter      ?? 135;
  const rInner      = opts.rInner      ?? 70;
  const depth       = opts.depth       ?? 22;
  const TILT        = opts.tilt        ?? (1 / 1.618);
  const showLabels  = opts.showLabels  ?? true;
  const LEADER_MAX  = opts.leaderMax   ?? (showLabels ? 70 : 0);
  const wallDarken  = opts.wallDarken  ?? 0.35;
  const innerDarken = opts.innerDarken ?? 0.45;
  const labelColor  = opts.labelColor  ?? '#1a1a1a';
  const valueColor  = opts.valueColor  ?? '#999';
  const labelSize   = opts.labelSize   ?? 13;
  const valueSize   = opts.valueSize   ?? 12;
  const padding     = opts.padding     ?? (showLabels ? 90 : 4);

  const W = 2 * rOuter + 2 * padding + 2 * LEADER_MAX;
  const H = 2 * rOuter * TILT + depth + 2 * padding * 0.5;
  const cx = W / 2;
  const cy = padding * 0.5 + rOuter * TILT;

  svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
  while (svg.firstChild) svg.removeChild(svg.firstChild);

  const NS = 'http://www.w3.org/2000/svg';

  const pt = (cy, r, a) => {
    const rad = (a - 90) * Math.PI / 180;
    return [cx + r * Math.cos(rad), cy + r * TILT * Math.sin(rad)];
  };

  const slicePath = (a0, a1) => {
    const large = a1 - a0 > 180 ? 1 : 0;
    const [x1, y1] = pt(cy, rOuter, a0);
    const [x2, y2] = pt(cy, rOuter, a1);
    const [x3, y3] = pt(cy, rInner, a1);
    const [x4, y4] = pt(cy, rInner, a0);
    return `M${x1},${y1} A${rOuter},${rOuter * TILT} 0 ${large} 1 ${x2},${y2} ` +
           `L${x3},${y3} A${rInner},${rInner * TILT} 0 ${large} 0 ${x4},${y4} Z`;
  };

  const wallPath = (r, b0, b1) => {
    const large = (b1 - b0) > 180 ? 1 : 0;
    const [x1, y1] = pt(cy,         r, b0);
    const [x2, y2] = pt(cy,         r, b1);
    const [x3, y3] = pt(cy + depth, r, b1);
    const [x4, y4] = pt(cy + depth, r, b0);
    return `M${x1},${y1} A${r},${r * TILT} 0 ${large} 1 ${x2},${y2} ` +
           `L${x3},${y3} A${r},${r * TILT} 0 ${large} 0 ${x4},${y4} Z`;
  };

  const darken = (hex, amt) => {
    const n = parseInt(hex.slice(1), 16);
    const r = Math.max(0, ((n >> 16) & 255) * (1 - amt)) | 0;
    const g = Math.max(0, ((n >>  8) & 255) * (1 - amt)) | 0;
    const b = Math.max(0, ( n        & 255) * (1 - amt)) | 0;
    return `rgb(${r},${g},${b})`;
  };

  const intersect = (a0, a1, b0, b1) => {
    const lo = Math.max(a0, b0), hi = Math.min(a1, b1);
    return hi > lo ? [lo, hi] : null;
  };

  const make = (tag, attrs) => {
    const el = document.createElementNS(NS, tag);
    for (const k in attrs) el.setAttribute(k, attrs[k]);
    return el;
  };

  const total = data.reduce((s, d) => s + d.value, 0);
  if (total <= 0) return;

  let a = 0;
  const slices = data.map(d => {
    const span = (d.value / total) * 360;
    const s = { ...d, a0: a, a1: a + span, mid: a + span / 2 };
    a += span;
    return s;
  });

  slices.forEach(s => {
    const r = intersect(s.a0, s.a1, 90, 270);
    if (!r) return;
    svg.appendChild(make('path', {
      d: wallPath(rOuter, r[0], r[1]),
      fill: darken(s.color, wallDarken)
    }));
  });

  slices.forEach(s => {
    [intersect(s.a0, s.a1, 0, 90), intersect(s.a0, s.a1, 270, 360)].forEach(r => {
      if (!r) return;
      svg.appendChild(make('path', {
        d: wallPath(rInner, r[0], r[1]),
        fill: darken(s.color, innerDarken)
      }));
    });
  });

  slices.forEach(s => {
    svg.appendChild(make('path', {
      d: slicePath(s.a0, s.a1),
      fill: s.color
    }));
  });

  if (!showLabels) return;

  slices.forEach(s => {
    const isFront  = s.mid > 90 && s.mid < 270;
    const anchorCy = isFront ? cy + depth : cy;

    const [lx, ly] = pt(anchorCy, rOuter + 2,  s.mid);
    const [ex, ey] = pt(anchorCy, rOuter + 28, s.mid);
    const side     = ex >= cx ? 1 : -1;

    const horizFrac = Math.abs(Math.sin(s.mid * Math.PI / 180));
    const ext       = LEADER_MAX * (1 - 0.5 * horizFrac);
    const textEnd   = ex + side * ext;
    const anchor    = side === 1 ? 'end' : 'start';

    svg.appendChild(make('polyline', {
      points: `${lx},${ly} ${ex},${ey} ${textEnd},${ey}`,
      fill: 'none',
      stroke: valueColor,
      'stroke-width': 1
    }));

    const name = make('text', {
      x: textEnd, y: ey - 4,
      'text-anchor': anchor,
      fill: labelColor,
      'font-size': labelSize,
      'font-weight': 500,
      'font-family': 'system-ui, -apple-system, sans-serif'
    });
    name.textContent = s.label;
    svg.appendChild(name);

    const pct = make('text', {
      x: textEnd, y: ey + 13,
      'text-anchor': anchor,
      fill: valueColor,
      'font-size': valueSize,
      'font-family': 'system-ui, -apple-system, sans-serif'
    });
    const pctVal = (s.value / total) * 100;
    pct.textContent = pctVal.toFixed(1) + '%';
    svg.appendChild(pct);
  });
}
