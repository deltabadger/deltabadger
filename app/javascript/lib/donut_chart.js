/**
 * Render a 3D-perspective donut chart into an <svg> element.
 *
 * @param {SVGElement} svg   Target SVG element. Its viewBox is set automatically.
 * @param {Array}      data  [{ label, value, color, symbol, logo }, ...]  (values in any unit)
 * @param {Object}     opts  Optional overrides (see source for list)
 */
export function renderDonut(svg, data, opts = {}) {
  const rOuter      = opts.rOuter      ?? 135;
  const rInner      = opts.rInner      ?? 70;
  const minDepth    = opts.minDepth    ?? 16;
  const depthStep   = opts.depthStep   ?? 0;
  const depth       = opts.depth       ?? (minDepth + depthStep * Math.max(data.length - 1, 0));
  const TILT        = opts.tilt        ?? (1 / 1.618);
  const showLabels  = opts.showLabels  ?? true;
  const tooltips    = opts.tooltips    ?? false;
  const LEADER_MAX  = opts.leaderMax   ?? (showLabels ? 70 : 0);
  const wallDarken  = opts.wallDarken  ?? 0.175;
  const innerDarken = opts.innerDarken ?? 0.225;
  const labelColor  = opts.labelColor  ?? '#1a1a1a';
  const valueColor  = opts.valueColor  ?? '#999';
  const labelSize   = opts.labelSize   ?? 10;
  const valueSize   = opts.valueSize   ?? 9;
  // Match the rest of the UI: theme --font-family (Montserrat by default), with fallbacks.
  const FONT        = opts.fontFamily  ?? "var(--font-family, 'Montserrat', sans-serif)";
  // Circular currency logo sits right next to each label (from slice.logo).
  const showLogos   = opts.showLogos   ?? true;
  const LOGO_R      = opts.logoRadius  ?? 9;
  const LOGO_GAP    = opts.logoGap     ?? 6;
  // Horizontal breathing room beyond the labels (keeps the left/right margins tight).
  const padX        = opts.padX        ?? (showLabels ? 24 : 4);
  const LABEL_HEIGHT = opts.labelHeight ?? Math.ceil(labelSize + valueSize + 7); // ≥2px gap between labels
  // Reserve a FIXED height for this many label rows per side, so the SVG never resizes as the
  // label count changes — it's always tall enough for the busiest (tallest) case.
  const reserveRows = opts.reserveRows ?? 13;
  // Floor so a de-collided label can't slide onto the donut: its anchored edge stays at
  // least this far outside the silhouette at its row (otherwise keep the natural position).
  const MIN_GAP     = opts.minGap      ?? 58;
  // Vertical clearance so the top label's name / bottom label's value never crop.
  const marginTop    = showLabels ? 14 : 4;
  const marginBottom = showLabels ? 14 : 4;
  const maxLabels   = opts.maxLabels   ?? Infinity; // cap label count; tail stays hoverable
  const otherThreshold = opts.otherThreshold ?? null; // fold slices with share < this into "Other"
  const otherLabel  = opts.otherLabel  ?? null;     // when set, fold the unlabeled tail into "Other"

  const rOuterY = rOuter * TILT;
  const W  = 2 * rOuter + 2 * LEADER_MAX + 2 * padX;
  const cx = W / 2;
  // H and cy are computed once the label set is known (below); the donut is centred in H.

  while (svg.firstChild) svg.removeChild(svg.firstChild);

  const NS = 'http://www.w3.org/2000/svg';

  const pt = (cyArg, r, a) => {
    const rad = (a - 90) * Math.PI / 180;
    return [cx + r * Math.cos(rad), cyArg + r * TILT * Math.sin(rad)];
  };

  const slicePath = (a0, a1, surfaceCy) => {
    const large = a1 - a0 > 180 ? 1 : 0;
    const [x1, y1] = pt(surfaceCy, rOuter, a0);
    const [x2, y2] = pt(surfaceCy, rOuter, a1);
    const [x3, y3] = pt(surfaceCy, rInner, a1);
    const [x4, y4] = pt(surfaceCy, rInner, a0);
    return `M${x1},${y1} A${rOuter},${rOuter * TILT} 0 ${large} 1 ${x2},${y2} ` +
           `L${x3},${y3} A${rInner},${rInner * TILT} 0 ${large} 0 ${x4},${y4} Z`;
  };

  const wallPath = (r, b0, b1, surfaceCy) => {
    const large = (b1 - b0) > 180 ? 1 : 0;
    const floorCy = cy + depth;
    const [x1, y1] = pt(surfaceCy, r, b0);
    const [x2, y2] = pt(surfaceCy, r, b1);
    const [x3, y3] = pt(floorCy,   r, b1);
    const [x4, y4] = pt(floorCy,   r, b0);
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

  const topSegments = slice => (
    [
      intersect(slice.a0, slice.a1, 0, 180),
      intersect(slice.a0, slice.a1, 180, 360)
    ].filter(Boolean).map(([a0, a1]) => ({ slice, a0, a1 }))
  );

  const make = (tag, attrs) => {
    const el = document.createElementNS(NS, tag);
    for (const k in attrs) el.setAttribute(k, attrs[k]);
    return el;
  };

  const total = data.reduce((s, d) => s + d.value, 0);
  if (total <= 0) return;

  const sliceDepths = new Array(data.length);
  data
    .map((d, index) => ({ value: d.value, index }))
    .sort((a, b) => a.value - b.value || a.index - b.index)
    .forEach(({ index }, rank) => {
      sliceDepths[index] = minDepth + rank * depthStep;
    });

  let a = 0;
  const slices = data.map((d, index) => {
    const span = (d.value / total) * 360;
    const s = { ...d, a0: a, a1: a + span, mid: a + span / 2, depth: sliceDepths[index] };
    a += span;
    return s;
  });

  // Thin the LEGEND (not the chart): fold small slices into one synthetic "Other" entry
  // (their summed %). Chosen by share threshold when given, else by count cap. Every slice
  // is still drawn and hoverable below — folded here so the per-side count drives the height.
  let labelEntries = slices;
  let otherEntry = null;
  if (showLabels && otherLabel) {
    const first = slices[0]; // the first slice always keeps its own label, even below threshold
    let rest = [];
    if (otherThreshold != null) {
      rest = slices.filter(s => s !== first && s.value / total < otherThreshold);
    } else if (maxLabels < slices.length) {
      const sorted = [...slices].sort((a, b) => b.value - a.value);
      rest = sorted.slice(maxLabels).filter(s => s !== first);
    }
    if (rest.length >= 2) {
      const restSet = new Set(rest);
      const restValue = rest.reduce((sum, r) => sum + r.value, 0);
      const a0s = Math.min(...rest.map(r => r.a0));
      const a1s = Math.max(...rest.map(r => r.a1));
      otherEntry = { label: otherLabel, value: restValue, mid: (a0s + a1s) / 2, isOther: true };
      labelEntries = slices.filter(s => !restSet.has(s)).concat(otherEntry);
    }
  }

  // Size the canvas: grow H so the busier side fits at full spacing (no crop, no overlap),
  // then centre the donut vertically.
  const donutH = 2 * rOuterY + depth + 32; // 16px donut margin top + bottom
  // Fixed height: reserve for `reserveRows` rows per side regardless of the current count,
  // so the chart never resizes when the number of coins changes (just more/less whitespace).
  const reservedH = showLabels ? (reserveRows - 1) * LABEL_HEIGHT + marginTop + marginBottom : 0;
  const H = Math.max(donutH, reservedH);
  const cy = (H - depth) / 2;
  svg.setAttribute('viewBox', `0 0 ${W} ${H}`);

  slices.forEach(s => { s.surfaceCy = cy + depth - s.depth; });
  if (otherEntry) otherEntry.surfaceCy = cy;

  const topPaintOrder = slices
    .flatMap(topSegments)
    .sort((a, b) => {
      const aIsRight = a.a0 < 180;
      const bIsRight = b.a0 < 180;
      if (aIsRight !== bIsRight) return aIsRight ? -1 : 1;
      return aIsRight ? a.a0 - b.a0 : b.a0 - a.a0;
    });

  topPaintOrder.forEach(({ slice, a0, a1 }) => {
    const outerWall = intersect(a0, a1, 90, 270);
    if (outerWall) {
      svg.appendChild(make('path', {
        d: wallPath(rOuter, outerWall[0], outerWall[1], slice.surfaceCy),
        fill: darken(slice.color, wallDarken)
      }));
    }

    [intersect(a0, a1, 0, 90), intersect(a0, a1, 270, 360)].forEach(r => {
      if (!r) return;
      svg.appendChild(make('path', {
        d: wallPath(rInner, r[0], r[1], slice.surfaceCy),
        fill: darken(slice.color, innerDarken)
      }));
    });

    const topAttrs = { d: slicePath(a0, a1, slice.surfaceCy), fill: slice.color };
    if (tooltips) {
      // Reuse the global `.ticker` hover-card: the controller resolves the asset from
      // `data-ticker-symbol` and appends the allocation from `data-ticker-note`.
      topAttrs.class = 'ticker';
      topAttrs['data-ticker-symbol'] = slice.symbol || slice.label || '';
      topAttrs['data-ticker-note'] = `${((slice.value / total) * 100).toFixed(2)}%`;
      topAttrs.style = 'cursor: pointer';
    }
    svg.appendChild(make('path', topAttrs));
  });

  if (!showLabels) return;

  // Connection point sits on the top ellipse for back slices and the bottom ellipse for
  // front ones, blended across a 20° zone around each 90°/270° crossing.
  const BLEND_DEG = 20;
  const bottomness = (mid) => {
    const h = BLEND_DEG / 2;
    if (mid >= 90 + h && mid <= 270 - h) return 1;
    if (mid <= 90 - h || mid >= 270 + h) return 0;
    return mid < 180 ? (mid - (90 - h)) / BLEND_DEG : 1 - (mid - (270 - h)) / BLEND_DEG;
  };

  // Natural label positions (one elbow per slice, leader run extended toward top/bottom).
  const labels = labelEntries.map(s => {
    const bottom   = bottomness(s.mid);
    const anchorCy = s.surfaceCy + bottom * depth;

    const [lx, ly] = pt(anchorCy, rOuter + 2,  s.mid);
    const [ex, ey] = pt(anchorCy, rOuter + 34, s.mid);
    const side     = ex >= cx ? 1 : -1;

    const horizFrac = Math.abs(Math.sin(s.mid * Math.PI / 180));
    const ext       = LEADER_MAX * (1 - 0.5 * horizFrac);
    const textEnd   = ex + side * ext;
    return { s, lx, ly, ex, ey, side, textEnd, labelY: ey, bottom };
  });

  // De-collide labels per side via pool-adjacent-violators: keep each label near its slice's
  // natural row, but pack overlapping clusters at LABEL_HEIGHT centred on the cluster mean.
  const bottomBound = H - marginBottom;
  [-1, 1].forEach(sideVal => {
    const group = labels.filter(l => l.side === sideVal).sort((a, b) => a.ey - b.ey);
    const n = group.length;
    if (!n) return;

    const blocks = [];
    for (let i = 0; i < n; i++) {
      let b = { start: i, sum: group[i].ey - i * LABEL_HEIGHT, count: 1 };
      b.mean = b.sum / b.count;
      while (blocks.length && blocks[blocks.length - 1].mean > b.mean) {
        const prev = blocks.pop();
        b = { start: prev.start, sum: prev.sum + b.sum, count: prev.count + b.count };
        b.mean = b.sum / b.count;
      }
      blocks.push(b);
    }
    blocks.forEach(b => {
      for (let i = b.start; i < b.start + b.count; i++) group[i].labelY = b.mean + i * LABEL_HEIGHT;
    });

    const available = bottomBound - marginTop;
    const span = group[n - 1].labelY - group[0].labelY;
    if (span > available) {
      const step = n > 1 ? available / (n - 1) : 0;
      group.forEach((l, i) => (l.labelY = marginTop + i * step));
    } else {
      const overTop = marginTop - group[0].labelY;
      const overBottom = group[n - 1].labelY - bottomBound;
      if (overTop > 0) group.forEach(l => (l.labelY += overTop));
      else if (overBottom > 0) group.forEach(l => (l.labelY -= overBottom));
    }
  });

  // Leader rule (X = slice's mid-edge point on the near/far ellipse; L = label's text line).
  // Diagonal connector only when it runs from L toward the donut without crossing past L,
  // otherwise a straight horizontal line that still reaches the rim at L.
  const rimXAtY = (y, sideX) => {
    let dy = 0;
    if (y < cy) dy = (cy - y) / rOuterY;
    else if (y > cy + depth) dy = (y - (cy + depth)) / rOuterY;
    return cx + sideX * rOuter * Math.sqrt(Math.max(0, 1 - dy * dy));
  };
  const rimTop = cy - rOuterY;
  const rimBottom = cy + depth + rOuterY;
  let logoSeq = 0; // unique clipPath ids per logo
  labels.forEach(l => {
    // Keep the natural angle-based text position, but never let it collapse onto the donut:
    // floor it to at least MIN_GAP outside the silhouette at the label's final row.
    const minOut = rimXAtY(l.labelY, l.side) + l.side * MIN_GAP;
    l.textEnd = l.side === 1 ? Math.max(l.textEnd, minOut) : Math.min(l.textEnd, minOut);

    const isUpper = l.bottom < 0.5; // X nearer the top ellipse than the bottom
    const inDonutY = l.labelY >= rimTop && l.labelY <= rimBottom;
    const original = !inDonutY || (isUpper ? l.ly > l.labelY : l.ly < l.labelY);
    const points = original
      ? `${l.lx},${l.ly} ${l.ex},${l.labelY} ${l.textEnd},${l.labelY}`
      : `${rimXAtY(l.labelY, l.side)},${l.labelY} ${l.textEnd},${l.labelY}`;
    svg.appendChild(make('polyline', {
      points,
      class: 'donut-leader',
      fill: 'none',
      stroke: valueColor,
      'stroke-width': 1
    }));

    const anchor = l.side === 1 ? 'end' : 'start';

    const name = make('text', {
      x: l.textEnd, y: l.labelY - 4,
      class: l.s.isOther ? 'donut-name donut-other' : 'donut-name',
      'text-anchor': anchor,
      fill: labelColor,
      'font-size': labelSize,
      'font-weight': 500,
      'font-family': FONT
    });
    name.textContent = l.s.label;
    svg.appendChild(name);

    const pct = make('text', {
      x: l.textEnd, y: l.labelY + valueSize + 1,
      class: 'donut-value',
      'text-anchor': anchor,
      fill: valueColor,
      'font-size': valueSize,
      'font-family': FONT
    });
    pct.textContent = ((l.s.value / total) * 100).toFixed(1) + '%';
    svg.appendChild(pct);

    // Circular currency logo right next to the label, just beyond its outer (anchored) edge,
    // centred on the leader line. Clipped to a circle; removed if the image fails to load.
    if (showLogos && l.s.logo && !l.s.isOther) {
      const gx = l.textEnd + l.side * (LOGO_GAP + LOGO_R);
      const gy = l.labelY;
      const clipId = `donut-logo-clip-${logoSeq++}`;
      const clip = make('clipPath', { id: clipId });
      clip.appendChild(make('circle', { cx: gx, cy: gy, r: LOGO_R }));
      svg.appendChild(clip);
      const img = make('image', {
        x: gx - LOGO_R, y: gy - LOGO_R, width: LOGO_R * 2, height: LOGO_R * 2,
        'clip-path': `url(#${clipId})`,
        preserveAspectRatio: 'xMidYMid slice',
        href: l.s.logo
      });
      img.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', l.s.logo);
      img.addEventListener('error', () => { img.remove(); clip.remove(); });
      svg.appendChild(img);
    }
  });
}
