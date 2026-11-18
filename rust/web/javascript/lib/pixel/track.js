/**
 * "Track portfolio" empty-state animation — procedural pixel art.
 *
 * A lookout tower rises out of the forest canopy (from the "Start investing"
 * animation). Only the plant tips and the top of the tower are visible; the
 * scene dissolves downward with an ordered-dither fade. On the platform the
 * hero keeps busy: peering through a telescope, typing at a screen with a
 * live stock chart, scribbling notes, and cheering when the chart goes up.
 * The antenna blinks and pings, the anemometer spins, the canopy sways.
 *
 * Usage:
 *   const anim = createTrackAnimation(canvasEl, { scale: 4, palette: 'mono', coins: 'stocks' });
 *   anim.destroy();
 */
export function createTrackAnimation(canvas, opts = {}) {
  const W = 160, H = 64;
  const DECK = 28;                 // platform surface (hero's feet stand on DECK - 1)
  const FADE_FROM = 43, FADE_TO = 64;   // tips stay crisp; stems and tower legs dissolve below (Deltabadger: to the bottom edge)
  const SPEED = 18;

  const PALETTES = {
    green: {},
    mono: { stem: '#2b4b7c', leaf: '#3d65a3', leafL: '#7397d2' },
  };
  const STOCK_HUES = [45, 140, 355, 268, 188, 24, 325, 215];

  const C = Object.assign({
    suit: '#e6edf9', suitShade: '#aebbd4', visor: '#1f3a6e', glint: '#9fd0ff', accent: '#3d63ff', pack: '#8a9bbd', boot: '#6c7fa8',
    coinO: '#c98a00', coinY: '#ffcc33', coinH: '#fff4b8', coinD: '#e8a600',
    stem: '#1f9d5a', leaf: '#2ecc71', leafL: '#8ef0b0',
    bg: '#011729', spark: '#ffffff',
    deck: '#4a6ea8', deckDark: '#2b4b7c', frame: '#34548a', rail: '#2a4268',
    metal: '#7f95c4', metalDark: '#4f6796', glass: '#cfe0ff',
    screen: '#071631', bezel: '#2b4b7c', up: '#2ecc71', down: '#ff5a6a',
    light: '#ff5a6a', paper: '#ffd27a', ink: '#3d63ff',
  }, PALETTES[opts.palette] || {}, opts.colors || {});

  // ---------- hero sprites (shared with the invest animation) ----------
  const HEAD = ['..wwvv..', '.wwvvgv.', '.wwvvvv.', '..wwvv..']; // helmet with a glass bubble front
  const ARMS = {
    free:  ['..aaaa..', 'pwwwwww.', 'pWwwwwW.', '..wwww..'],
    reach: ['..aaaa..', 'pwwwwwWW', 'pwwwww..', '..wwww..'],
    raise: ['..aaaaW.', 'pwwwwwW.', 'pwwwww..', '..wwww..'],
  };
  const CHEER = ['W.wwvv.W', 'wwwvvgvw', 'wwwvvvvw', 'w.wwvv.w', '..aaaa..', 'pwwwwww.', 'pwwwwww.', '..wwww..'];
  const LEGS = [
    ['..W..W..', '..B..B..'],
    ['..W.W...', '.B...B..'],
    ['..W..W..', '..B..B..'],
    ['...W.W..', '..B...B.'],
  ];
  const HERO_MAP = { w: C.suit, W: C.suitShade, v: C.visor, g: C.glint, a: C.accent, p: C.pack, B: C.boot };

  // ---------- canvas ----------
  const scale = opts.scale || 4;
  canvas.width = W; canvas.height = H;
  canvas.style.width = W * scale + 'px';
  canvas.style.height = H * scale + 'px';
  canvas.style.imageRendering = 'pixelated';
  const ctx = canvas.getContext('2d');

  // ordered-dither fade toward the bottom
  const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
  function px(x, y, c) {
    x = Math.floor(x); y = Math.floor(y);
    if (x < 0 || x >= W || y < 0 || y >= H) return;
    if (y > FADE_FROM) {
      const t = (y - FADE_FROM) / (FADE_TO - FADE_FROM);
      if ((BAYER[(y & 3) * 4 + (x & 3)] + 0.5) / 16 < t) return;
      // Deltabadger: the colour fades into the page as well, not only the dither
      if (/^#[0-9a-f]{6}$/i.test(c)) c = mix(c, C.bg, t * 0.85);
    }
    ctx.fillStyle = c;
    ctx.fillRect(x, y, 1, 1);
  }
  function drawRows(rows, x, y, map, flip = false) {
    for (let r = 0; r < rows.length; r++)
      for (let c = 0; c < rows[r].length; c++) {
        const col = map[rows[r][c]];
        if (col) px(x + (flip ? rows[r].length - 1 - c : c), y + r, col);
      }
  }
  function line(x0, y0, x1, y1, c) {
    const n = Math.max(Math.abs(x1 - x0), Math.abs(y1 - y0));
    for (let i = 0; i <= n; i++)
      px(Math.round(x0 + (x1 - x0) * i / n), Math.round(y0 + (y1 - y0) * i / n), c);
  }

  // ---------- rng + colour helpers ----------
  function mulberry32(a) {
    return function () {
      a |= 0; a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }
  const rng = mulberry32(opts.seed ?? (Date.now() & 0xffffff));

  const DEPTHS = 4;
  const DEPTH_BACK = opts.depthBack ?? 0.6, DEPTH_FRONT = opts.depthFront ?? 0.25;
  const hex = c => [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16));
  const mix = (a, b, t) => '#' + hex(a).map((v, i) => Math.round(v + (hex(b)[i] - v) * t)
    .toString(16).padStart(2, '0')).join('');
  const depthTone = (c, d) => d < 0.5
    ? mix(c, C.bg, (0.5 - d) * DEPTH_BACK * 2)
    : mix(c, '#ffffff', (d - 0.5) * DEPTH_FRONT * 2);

  const GOLD = { o: C.coinO, y: C.coinY, h: C.coinH, d: C.coinD };
  const hueMap = hue => ({
    o: `hsl(${hue},70%,34%)`, y: `hsl(${hue},85%,58%)`,
    h: `hsl(${hue},100%,86%)`, d: `hsl(${hue},80%,47%)`,
  });
  const HUES = opts.coins === 'stocks' ? STOCK_HUES : Array.isArray(opts.coins) ? opts.coins : null;
  const COIN_MAPS = HUES ? HUES.map(hueMap) : [GOLD];

  // ---------- canopy (tops of the forest) ----------
  const BASE = H + 14;             // plant roots sit below the frame
  const TIP_STYLES = ['spike', 'fork', 'berries', 'crown', 'cross'];
  const plants = [];
  for (let x = 4; x < W - 2; x += 8) {                 // evenly spaced canopy
    for (let layer = 0; layer < 1; layer++) {
      const depth = Math.floor(rng() * DEPTHS) / (DEPTHS - 1);
      let top = 33 + Math.floor(rng() * 6) - Math.round(depth * 2); // Deltabadger: 3 px taller than the draft
      if (x > 54 && x < 106) top = Math.max(top, DECK + 12); // keep the platform clear
      const r = mulberry32(Math.floor(rng() * 1e9));
      const leaves = [];
      let side = rng() < 0.5 ? -1 : 1;
      for (let y = 3 + Math.floor(rng() * 3); y < BASE - top - 1; y += 3 + Math.floor(r() * 2)) {
        leaves.push({ y, side, len: 2 + Math.floor(r() * 2) });
        side = -side;
      }
      plants.push({
        x: x + Math.floor(rng() * 3) - 1, h: BASE - top, depth, leaves,
        phase: rng() * Math.PI * 2, freq: 0.12 + rng() * 0.2, curve: 1 + rng() * 2,
        coin: COIN_MAPS[Math.floor(rng() * COIN_MAPS.length)],
        tip: TIP_STYLES[Math.floor(rng() * TIP_STYLES.length)],
        spikeMax: 3, maturity: -1,                         // simple, uniform spike tips
        dots: [[2, 1], [1, -2], [3, -1]].sort(() => rng() - 0.5),
        accent: false,
        stem: depthTone(C.stem, depth), leafC: depthTone(C.leaf, depth), leafTip: depthTone(C.leafL, depth),
      });
    }
  }
  plants.sort((a, b) => a.depth - b.depth);

  function drawPlant(p) {
    const xo = i => {
      const f = Math.min(1, i / 24);
      return Math.round(Math.sin(i * p.freq + p.phase) * p.curve * f +
        Math.sin(clock * 1.4 + p.phase) * 1.6 * f * f);
    };
    for (let i = 0; i < p.h; i++) px(p.x + xo(i), BASE - 1 - i, p.stem);
    for (const L of p.leaves) {
      if (L.y >= p.h - 1) continue;
      const bx = p.x + xo(L.y), by = BASE - 1 - L.y;
      for (let k = 1; k <= L.len; k++)
        px(bx + L.side * k, by - (k >= 2 ? 1 : 0),
          k < L.len ? p.leafC : p.accent && L.y % 2 ? p.coin.d : p.leafTip);
    }
    drawTip(p, p.x + xo(p.h - 1), BASE - p.h);
  }

  function drawTip(p, tx, ty) {
    const c = p.coin;
    const glow = Math.sin(clock * 1.7 + p.phase) > 0.55 ? C.spark : c.h;
    const len = p.spikeMax;
    const grad = [c.o, c.d, c.d, c.y, c.y, c.h];
    for (let k = 1; k <= len; k++)
      px(tx, ty - k, k === len ? glow : grad[Math.min(grad.length - 2, k - 1 + 6 - len)]);
    const top = ty - len, m = p.maturity;
    if (m < 0) return twinkle(tx, top);
    const grow = (step, max) => Math.min(max, 1 + Math.floor(m / step));
    switch (p.tip) {
      case 'fork': {
        const n = grow(8, 3);
        for (let k = 1; k <= n; k++) { const col = k === n ? c.h : c.y; px(tx - k, top - k, col); px(tx + k, top - k, col); }
        break;
      }
      case 'berries': {
        const n = grow(7, p.dots.length);
        for (let i = 0; i < n; i++) { const [dx, dy] = p.dots[i], col = i % 2 ? c.h : c.y; px(tx - dx, top + dy, col); px(tx + dx, top + dy, col); }
        break;
      }
      case 'crown':
        px(tx - 1, top + 1, c.d); px(tx + 1, top + 1, c.d);
        if (m > 8) { px(tx - 1, top, c.y); px(tx + 1, top, c.y); }
        if (m > 18) { px(tx - 2, top - 1, c.h); px(tx + 2, top - 1, c.h); }
        break;
      case 'cross': {
        const n = grow(9, 3);
        for (let k = 0; k < n; k++) { const yy = top + 1 + k * 2, col = k === 0 ? c.h : c.y; px(tx - 1, yy, col); px(tx + 1, yy, col); }
        break;
      }
    }
    twinkle(tx, top);
  }
  function twinkle(tx, top) {
    // rare one-pixel glint (uses the plant phase via closure-free hash)
    if ((clock * 0.3 + tx * 0.37) % 1 < 0.04) px(tx, top - 2, C.spark);
  }

  // ---------- tower ----------
  const CX = 80, DL = 58, DR = 102;          // deck left / right
  function drawTower() {
    // legs, tapering outward as they go down into the canopy
    const legs = [[-14, -24], [-6, -9], [6, 9], [14, 24]];
    for (const [a, b] of legs) line(CX + a, DECK + 2, CX + b, H + 4, C.frame);
    // cross bracing between the outer and inner legs
    const at = (a, b, y) => Math.round(CX + a + (b - a) * (y - DECK - 2) / (H + 2 - DECK));
    for (let y = DECK + 4, i = 0; y < H; y += 9, i++) {
      const y2 = y + 9;
      for (const [o, n] of [[legs[0], legs[1]], [legs[3], legs[2]]]) {
        line(at(...o, y), y, at(...n, y), y, C.rail);
        if (i % 2) line(at(...o, y), y, at(...n, y2), y2, C.rail);
        else line(at(...n, y), y, at(...o, y2), y2, C.rail);
      }
      line(at(...legs[1], y), y, at(...legs[2], y), y, C.rail);
    }
    // deck
    for (let x = DL; x <= DR; x++) { px(x, DECK, C.deck); px(x, DECK + 1, C.deckDark); }
    px(DL - 1, DECK, C.deckDark); px(DR + 1, DECK, C.deckDark);
  }
  function drawRailing() {
    for (let x = DL; x <= DR; x++) px(x, DECK - 4, C.rail);
    for (let x = DL; x <= DR; x += 6) for (let y = DECK - 3; y < DECK; y++) px(x, y, C.rail);
  }

  // ---------- equipment ----------
  const chart = [];
  const POINTS = 8;                          // columns SX+1..SX+8; SX+9 stays empty
  let chartV = 0;
  for (let i = 0; i < POINTS; i++) chart.push(Math.round(i * 0.5));
  chartV = chart[POINTS - 1];
  function tickChart() {
    chartV += Math.round((rng() - 0.3) * 2.2);  // random walk, mostly up
    chart.shift(); chart.push(chartV);
  }
  const SX = 88, SY = 12;                    // screen top-left (11 x 9)

  function drawEquipment() {
    // antenna with blinking light and occasional "ping"
    const AX = 61, AT = 11;
    for (let y = AT + 1; y < DECK; y++) px(AX, y, C.metalDark);
    px(AX - 1, AT + 3, C.metalDark); px(AX + 1, AT + 3, C.metalDark);
    // Deltabadger: sometimes a pulse of light climbs the mast first, from nothing at the deck to full at the tip.
    if (ant.pulse >= 0) {
      const p = ant.pulse / PULSE, y = Math.round(DECK - 1 - p * (DECK - 2 - AT));
      px(AX, y, mix(C.bg, C.light, p));
      if (y + 1 < DECK) px(AX, y + 1, mix(C.bg, C.light, p * 0.4));
    }
    px(AX, AT, ant.ping >= 0 && ant.ping < 0.15 ? C.light : Math.sin(clock * 4) > 0 ? C.light : C.rail);
    // Deltabadger: the ring is faint, and fainter as it spreads.
    if (ant.ping >= 0) {
      const ping = ant.ping / 0.9;
      const r = 2 + Math.floor(ping * 3) * 2, col = mix(C.metal, C.bg, 0.65 + ping * 0.3);
      for (const s of [-1, 1]) { px(AX + s * r, AT - 1, col); px(AX + s * r, AT, col); px(AX + s * (r - 1), AT - 2, col); px(AX + s * (r - 1), AT + 1, col); }
    }

    // telescope on a tripod, pointing up-left
    px(66, DECK - 6, C.metalDark);
    line(66, DECK - 5, 66, DECK - 1, C.metalDark);
    line(66, DECK - 5, 63, DECK - 1, C.metalDark);
    line(66, DECK - 5, 69, DECK - 1, C.metalDark);
    line(68, DECK - 7, 62, DECK - 11, C.metal);
    line(67, DECK - 8, 62, DECK - 12, C.metal);
    px(68, DECK - 8, C.metalDark);                         // eyepiece
    px(61, DECK - 12, Math.sin(clock * 0.9) > 0.8 ? C.spark : C.glass); // lens glint

    // screen on a stand with a live chart
    line(SX + 5, SY + 9, SX + 5, DECK - 1, C.metalDark);
    line(SX + 3, DECK - 1, SX + 7, DECK - 1, C.metalDark);
    for (let x = SX; x < SX + 11; x++) for (let y = SY; y < SY + 9; y++)
      px(x, y, x === SX || x === SX + 10 || y === SY || y === SY + 8 ? C.bezel : C.screen);
    // the view follows the newest point (kept two rows below the top), so the
    // line only ever scrolls - its shape never changes
    const yOf = v => SY + 3 - (v - chart[POINTS - 1]);
    for (let i = 0; i < POINTS; i++) {
      const x = SX + 1 + i, y = yOf(chart[i]);
      const prev = i ? yOf(chart[i - 1]) : y;
      const col = i && chart[i] < chart[i - 1] ? C.down : C.up;
      for (let yy = Math.min(y, prev); yy <= Math.max(y, prev); yy++)
        if (yy >= SY + 2 && yy <= SY + 7) px(x, yy, i === POINTS - 1 && yy === y ? C.spark : col);
    }

    // anemometer
    const MX = 101, MT = 7;
    for (let y = MT + 1; y < DECK; y++) px(MX, y, C.metalDark);
    const f = Math.floor(clock * 10) % 4;
    const arms = [[[-2, 0], [2, 0]], [[-2, -1], [2, 1]], [[0, -2], [0, 2]], [[-2, 1], [2, -1]]][f];
    px(MX, MT, C.metal);
    for (const [dx, dy] of arms) {
      px(MX + Math.sign(dx), MT + Math.sign(dy), C.metal);
      px(MX + dx, MT + dy, C.glass);
    }
  }

  // ---------- hero tasks ----------
  const SPOTS = {
    scope:  { x: 67, face: 'left',  dur: [2.5, 4] },
    screen: { x: 79, face: 'right', dur: [3, 5] },
    notes:  { x: 72, face: null,    dur: [2, 3.5] },
  };
  let hero = { x: 72, faceLeft: true }, task = 'notes', phase = 'work', phaseT = 0, workFor = 2;
  let chartT = 0, cheered = false, clock = 0;

  function nextTask() {
    const options = Object.keys(SPOTS).filter(k => k !== task);
    task = options[Math.floor(rng() * options.length)];
    phase = 'walk'; phaseT = 0;
    const s = SPOTS[task];
    workFor = s.dur[0] + rng() * (s.dur[1] - s.dur[0]);
    cheered = false;
  }

  // Deltabadger: the antenna's rhythm. Every few seconds it pings; now and then a pulse climbs the mast first and the
  // ping goes out when it reaches the tip.
  const PULSE = 0.7;
  const ant = { t: 0, next: 1.5, pulse: -1, ping: -1 };
  function updateAntenna(dt) {
    ant.t += dt;
    if (ant.ping >= 0 && (ant.ping += dt) > 0.9) ant.ping = -1;
    if (ant.pulse >= 0 && (ant.pulse += dt) >= PULSE) { ant.pulse = -1; ant.ping = 0; }
    if (ant.t >= ant.next) {
      ant.t = 0; ant.next = 2.6 + rng() * 1.6;
      if (rng() < 0.4) ant.pulse = 0; else ant.ping = 0;
    }
  }

  function update(dt) {
    clock += dt; phaseT += dt; chartT += dt;
    updateAntenna(dt);
    if (chartT > (task === 'screen' && phase === 'work' ? 0.9 : 1.8)) { chartT = 0; tickChart(); }

    if (phase === 'walk') {
      const target = SPOTS[task].x, d = target - hero.x;
      hero.faceLeft = d < 0;
      if (Math.abs(d) <= SPEED * dt) {
        hero.x = target; phase = 'work'; phaseT = 0;
        const f = SPOTS[task].face;
        hero.faceLeft = f ? f === 'left' : rng() < 0.5;
      } else hero.x += Math.sign(d) * SPEED * dt;
    } else if (phase === 'work') {
      if (phaseT > workFor) {
        // a good day on the chart? celebrate before moving on
        if (task === 'screen' && !cheered && chart[POINTS - 1] > chart[0]) { phase = 'cheer'; phaseT = 0; cheered = true; }
        else nextTask();
      }
    } else if (phase === 'cheer' && phaseT > 0.8) nextTask();
  }

  function drawHero() {
    const walking = phase === 'walk';
    const legF = walking ? Math.floor(clock * 8) % 4 : 0;
    const bob = walking && legF % 2 === 0 ? -1 : 0;
    let upper, crouch = 0, hop = 0;

    if (phase === 'cheer') {
      upper = CHEER;
      hop = -Math.round(Math.abs(Math.sin(phaseT / 0.8 * 2 * Math.PI)) * 3);
    } else if (walking) upper = HEAD.concat(ARMS.free);
    else if (task === 'scope') { upper = HEAD.concat(ARMS.raise); crouch = 1; }
    else if (task === 'screen') upper = HEAD.concat(Math.floor(clock * 6) % 2 ? ARMS.reach : ARMS.raise);
    else upper = HEAD.concat(ARMS.reach);

    const x = Math.round(hero.x), y = DECK - 10 + crouch + hop + bob;
    drawRows(upper.concat(LEGS[legF]), x, y, HERO_MAP, hero.faceLeft);

    if (!walking && phase === 'work' && task === 'notes') {     // clipboard + scribbling pencil
      const cx = hero.faceLeft ? x - 2 : x + 8;
      for (let yy = y + 4; yy < y + 8; yy++) { px(cx, yy, C.paper); px(cx + 1, yy, C.paper); }
      px(cx + (Math.floor(clock * 5) % 2), y + 5 + (Math.floor(clock * 3) % 2), C.ink);
    }
  }

  // ---------- frame ----------
  function render() {
    ctx.clearRect(0, 0, W, H);
    const back = plants.filter(p => p.depth < 0.5), front = plants.filter(p => p.depth >= 0.5);
    back.forEach(drawPlant);
    drawTower();
    front.forEach(drawPlant);
    drawRailing();
    drawEquipment();
    drawHero();
  }

  let last = performance.now(), raf;
  function frame(now) {
    const dt = Math.max(0, Math.min(0.1, (now - last) / 1000));
    for (let i = 0; i < (opts.timeScale || 1); i++) update(dt);
    last = now;
    render();
    raf = requestAnimationFrame(frame);
  }
  raf = requestAnimationFrame(frame);

  // Deltabadger: a scheme change recolours the scene in place.
  function recolor(colors) {
    Object.assign(C, colors);
    for (const p of plants) {
      p.stem = depthTone(C.stem, p.depth);
      p.leafC = depthTone(C.leaf, p.depth);
      p.leafTip = depthTone(C.leafL, p.depth);
    }
  }

  return { recolor, destroy() { cancelAnimationFrame(raf); } };
}

