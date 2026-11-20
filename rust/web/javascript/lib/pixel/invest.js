/**
 * "Start investing" empty-state animation — procedural pixel art.
 *
 * A little astronaut hero walks back and forth across the ground carrying a spinning coin, tosses it into the ground,
 * cheers, and walks on to the edge, where he turns back with a new coin (Deltabadger: he never leaves the scene).
 * Every coin sprouts a procedurally generated plant
 * (random curve, leaf layout, growth speed, sway) that grows forever and
 * eventually leaves the top of the frame. The hero plants the free spots nearest
 * the middle, then further out to the sides, so the garden slowly turns into a forest.
 *
 * Usage:
 *   const anim = createInvestAnimation(canvasEl, { scale: 4 });
 *   anim.destroy();
 *
 * The canvas is drawn at a tiny logical resolution (160x64) and scaled up
 * with CSS `image-rendering: pixelated`, so it stays crisp at any size.
 */
const PALETTES = {
  green: {},
  // plants in the same blue family as the ground line and the UI text
  mono: {
    stem: '#2b4b7c', leaf: '#3d65a3', leafL: '#7397d2',
    deadStem: '#2a3142', deadLeaf: '#3a4256',
  },
};
// hues for coins: 'stocks' (gold, green, red, violet, cyan, orange, pink, blue)
const STOCK_HUES = [45, 140, 355, 268, 188, 24, 325, 215];

export function createInvestAnimation(canvas, opts = {}) {
  // Deltabadger: the frame is 64 rows tall in the layout, but the canvas reaches up to the window's top edge, so the
  // plants grow on above it (`fit`): H and GROUND follow.
  const W = 160, ROWS = 64;
  let H = ROWS, GROUND = H - 1;
  const SPEED = 24;       // hero walk speed, logical px / s
  const GROWTH = opts.growth || 0.12;   // average stem growth, logical px / s
  const EDGE = [2, W - 10];             // Deltabadger: the hero turns back here, still in the frame
  const SPACING = [20, 12, 7, 4];       // gap-filling passes, wide -> dense
  const DIE_CHANCE = opts.dieChance ?? 0.2; // share of plants that wilt and vanish
  const WILT = 1.6, SHRINK = 1.4;       // seconds: turn brown, then sink away

  const C = Object.assign({
    suit: '#e6edf9', suitShade: '#aebbd4', visor: '#1f3a6e', glint: '#9fd0ff', accent: '#3d63ff', pack: '#8a9bbd', boot: '#6c7fa8',
    coinO: '#c98a00', coinY: '#ffcc33', coinH: '#fff4b8', coinD: '#e8a600',
    stem: '#1f9d5a', leaf: '#2ecc71', leafL: '#8ef0b0',

    deadStem: '#5c5640', deadLeaf: '#7d7350',
    bg: '#011729', groundTop: '#1c3358', dust: '#3a5a8c', spark: '#ffffff',
  }, PALETTES[opts.palette] || {}, opts.colors || {});

  // ---------- sprites (8px wide hero) ----------
  const HEAD = ['..wwvv..', '.wwvvgv.', '.wwvvvv.', '..wwvv..']; // helmet with a glass bubble front
  const ARMS = {
    free:  ['..aaaa..', 'pwwwwww.', 'pWwwwwW.', '..wwww..'],
    carry: ['..aaaa..', 'pwwwwwWW', 'pwwwww..', '..wwww..'],
  };
  const CHEER = ['W.wwvv.W', 'wwwvvgvw', 'wwwvvvvw', 'w.wwvv.w', '..aaaa..', 'pwwwwww.', 'pwwwwww.', '..wwww..'];
  const LEGS = [
    ['..W..W..', '..B..B..'],
    ['..W.W...', '.B...B..'],
    ['..W..W..', '..B..B..'],
    ['...W.W..', '..B...B.'],
  ];
  const HERO_MAP = { w: C.suit, W: C.suitShade, v: C.visor, g: C.glint, a: C.accent, p: C.pack, B: C.boot };

  const COIN = [
    ['.ooo.', 'ohyyo', 'oyyyo', 'oyydo', '.ooo.'],
    ['.o.', 'oho', 'oyo', 'oyo', '.o.'],
    ['o', 'h', 'y', 'd', 'o'],
  ];
  const SPIN = [0, 1, 2, 1];
  const GOLD = { o: C.coinO, y: C.coinY, h: C.coinH, d: C.coinD };
  const hueMap = hue => ({
    o: `hsl(${hue},70%,34%)`, y: `hsl(${hue},85%,58%)`,
    h: `hsl(${hue},100%,86%)`, d: `hsl(${hue},80%,47%)`,
  });
  const HUES = opts.coins === 'stocks' ? STOCK_HUES : Array.isArray(opts.coins) ? opts.coins : null;
  const COIN_MAPS = HUES ? HUES.map(hueMap) : [GOLD];

  // ---------- canvas ----------
  const scale = opts.scale || 4;
  let ctx, el;
  // Deltabadger: the scene can move to another canvas (the bots page drawn again) and go on where it was.
  function setup(canvas) {
    el = canvas; H = 0;
    canvas.style.width = W * scale + 'px';
    canvas.style.imageRendering = 'pixelated';
    size(ROWS, scale);
    fit();
  }
  function size(rows, s) {
    H = rows; GROUND = H - 1;
    el.width = W; el.height = H;
    el.style.height = H * scale + 'px';
    el.style.marginTop = -(H - ROWS) * s + 'px';
    ctx = el.getContext('2d');
    ctx.imageSmoothingEnabled = false;
  }
  // Deltabadger: as many rows as reach from the ground line up to the window's top edge, at the scale the canvas is drawn
  // (a narrow page shrinks it). The layout keeps 64 rows; the ones over that hang above on a negative margin, so the
  // canvas's bottom edge (the ground) stays where the layout put it. Checked every frame, as anything moving the page
  // (a font loading, a resize) moves the ground line; the canvas is written only on a change.
  function fit() {
    const r = el.getBoundingClientRect(), s = r.width / W;
    if (!s) return;
    const rows = Math.max(ROWS, Math.ceil(r.bottom / s));
    if (rows !== H) size(rows, s);
  }
  setup(canvas);

  let clip = false;
  function px(x, y, c) {
    x = Math.floor(x); y = Math.floor(y);
    if (clip && y >= GROUND) return;
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
  function drawCoin(cx, top, frame, map = GOLD) {
    const rows = COIN[frame];
    drawRows(rows, cx - (rows[0].length >> 1), top, map);
  }

  // ---------- rng ----------
  function mulberry32(a) {
    return function () {
      a |= 0; a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  // depth shading: back plants sink toward the background, front ones lift
  const DEPTHS = 4;
  const DEPTH_BACK = opts.depthBack ?? 0.6;   // furthest plants: 60% of the way to bg
  const DEPTH_FRONT = opts.depthFront ?? 0.25; // nearest plants: 25% of the way to white
  const hex = c => [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16));
  const mix = (a, b, t) => '#' + hex(a).map((v, i) => Math.round(v + (hex(b)[i] - v) * t)
    .toString(16).padStart(2, '0')).join('');
  const depthTone = (c, d) => d < 0.5
    ? mix(c, C.bg, (0.5 - d) * DEPTH_BACK * 2)       // back: toward the background
    : mix(c, '#ffffff', (d - 0.5) * DEPTH_FRONT * 2); // front: toward white

  // ground: a single line that fades out at the edges with a dither
  const FADE = 16;
  function drawGround() {
    for (let x = 0; x < W; x++) {
      const e = Math.min(x, W - 1 - x);
      if (e >= FADE || ((x * 5) % FADE) < e) px(x, GROUND, C.groundTop);
    }
  }

  // ---------- state ----------
  const rng = mulberry32(opts.seed ?? (Date.now() & 0xffffff));
  let plants = [], particles = [], phase, phaseT, hero, coin, spot;
  let clock = 0;

  // The free spot nearest the middle at the widest spacing that still fits, so the garden spreads from the centre out
  // to the sides; when the ground is full, a new sprout replaces whatever is growing there.
  function pickSpot() {
    for (const gap of SPACING) {
      const free = [];
      for (let i = 16; i <= W - 16; i++) if (plants.every(p => Math.abs(p.x - i) >= gap)) free.push(i);
      free.sort((a, b) => Math.abs(a - W / 2) - Math.abs(b - W / 2));
      if (free.length) return free[Math.min(free.length - 1, Math.floor(rng() * 3))];
    }
    return 16 + Math.floor(rng() * (W - 32));
  }

  function makePlant(x) {
    const depth = Math.floor(rng() * DEPTHS) / (DEPTHS - 1); // 0 = back, 1 = front
    const r = mulberry32(Math.floor(rng() * 1e9));
    return {
      x, t0: clock, r, coin: hero.coin,
      speed: GROWTH * (0.35 + rng() * 1.6),       // some crawl, some shoot up
      dieAt: rng() < DIE_CHANCE ? 8 + rng() * 70 : Infinity, // seconds of life
      leaves: [], nextLeaf: 3, side: rng() < 0.5 ? -1 : 1,
      phase: rng() * Math.PI * 2,
      freq: 0.12 + rng() * 0.2,
      curve: 1 + rng() * 2,
      tip: TIP_STYLES[Math.floor(rng() * TIP_STYLES.length)],
      spikeMax: 3 + Math.floor(rng() * 4),
      dots: [[2, 1], [1, -2], [3, -1]].sort(() => rng() - 0.5),
      accent: rng() < 0.35,                            // some get coloured leaf tips
      depth,
      stem: depthTone(C.stem, depth),
      leaf: depthTone(C.leaf, depth),
      leafTip: depthTone(C.leafL, depth),
      bareCap: 20 + Math.floor(rng() * 8),            // leaves never shed above this
    };
  }
  // stem height: quick sprout, then slow endless growth
  const heightOf = p => {
    const age = Math.min(clock - p.t0, p.dieAt);
    return Math.floor(Math.min(5, age * 2.5) + age * p.speed);
  };

  // Deltabadger: the hero turns at an edge with a new coin and sets out for the next spot. `dir`: -1 walks left.
  // The coin is held 3px before the sprite, on whichever side it faces.
  const holdAt = () => (hero.dir < 0 ? -3 : 10);
  function newCoin() { hero.pose = 'carry'; hero.coin = COIN_MAPS[Math.floor(rng() * COIN_MAPS.length)]; }
  function startWalk() {
    spot = pickSpot();
    coin = null;
    go('walkIn');
  }
  function go(p) { phase = p; phaseT = 0; }

  function update(dt) {
    clock += dt; phaseT += dt;

    switch (phase) {
      case 'walkIn': {
        hero.x += hero.dir * SPEED * dt;
        const target = spot - holdAt();
        if ((hero.x - target) * hero.dir >= 0) { hero.x = target; go('kneel'); }
        break;
      }
      case 'kneel':
        if (phaseT > 0.35) {
          hero.pose = 'free';
          coin = { x: spot, y: GROUND - 6, vy: -70, landed: false, sink: 0 };
          go('drop');
        }
        break;
      case 'drop':
        if (!coin.landed) {
          coin.vy += 260 * dt;
          coin.y += coin.vy * dt;
          if (coin.y + 5 >= GROUND && coin.vy > 0) { coin.y = GROUND - 5; coin.landed = true; }
        } else {
          coin.sink += dt * 12;
          if (coin.sink >= 5) {
            plants = plants.filter(p => Math.abs(p.x - coin.x) >= 3);
            plants.push(makePlant(coin.x));
            for (let i = 0; i < 7; i++)
              particles.push({
                x: coin.x, y: GROUND - 1,
                vx: (rng() - 0.5) * 50, vy: -25 - rng() * 35, life: 0.7,
              });
            coin = null;
            hero.pose = 'cheer';
            go('cheer');
          }
        }
        break;
      case 'cheer':
        if (phaseT > 0.8) { hero.pose = 'free'; go('walkOut'); }
        break;
      case 'walkOut': // on to the edge ahead
        hero.x += hero.dir * SPEED * dt;
        if (hero.x <= EDGE[0] || hero.x >= EDGE[1]) {
          hero.x = Math.min(EDGE[1], Math.max(EDGE[0], hero.x));
          hero.dir = -hero.dir;
          newCoin();
          go('turn');
        }
        break;
      case 'turn': // a beat with the new coin before setting out
        if (phaseT > 0.5) startWalk();
        break;
    }

    plants = plants.filter(p => clock - p.t0 < p.dieAt + WILT + SHRINK);

    for (const p of particles) {
      p.vy += 160 * dt; p.x += p.vx * dt; p.y += p.vy * dt; p.life -= dt;
    }
    particles = particles.filter(p => p.life > 0 && p.y < GROUND);
  }

  // ---------- drawing ----------
  function drawPlant(p) {
    const dying = clock - p.t0 - p.dieAt;      // > 0 once the plant is dying
    const stemC = dying > 0 ? C.deadStem : p.stem;
    const leafC = dying > 0 ? C.deadLeaf : p.leaf;
    const tipC = dying > 0 ? C.deadLeaf : p.leafTip;
    let h = heightOf(p);
    if (dying > WILT) h = Math.floor(h * Math.max(0, 1 - (dying - WILT) / SHRINK));
    if (h < 1) return; // Deltabadger: no mound of soil; a stem rises straight from the ground line

    while (p.nextLeaf < h) { // leaves are generated lazily as the stem grows
      p.leaves.push({ y: p.nextLeaf, side: p.side, len: 2 + Math.floor(p.r() * 2) });
      p.side = -p.side;
      p.nextLeaf += 3 + Math.floor(p.r() * 2);
    }

    const xo = i => {
      const f = Math.min(1, i / 24);
      return Math.round(
        Math.sin(i * p.freq + p.phase) * p.curve * f +
        Math.sin(clock * 1.4 + p.phase) * 1.6 * f * f +
        (dying > 0 ? Math.min(1, dying / WILT) * 3 * f * f * Math.sign(Math.sin(p.phase) || 1) : 0) // droop
      );
    };

    const top = Math.min(h, GROUND + 8); // nothing above the frame is drawn
    for (let i = 0; i < top; i++) px(p.x + xo(i), GROUND - 1 - i, stemC);

    const bare = Math.min(p.bareCap, Math.max(0, (h - 18) * 0.55)); // taller trunk -> bare lower stem, capped
    for (const L of p.leaves) {
      if (L.y >= h - 1 || L.y > top) continue;
      const n = Math.min(L.len, 1 + ((h - L.y) >> 1), Math.floor(L.y - bare) + 1);
      if (n < 1) continue;
      const bx = p.x + xo(L.y), by = GROUND - 1 - L.y;
      for (let k = 1; k <= n; k++)
        px(bx + L.side * k, by - (k >= 2 ? 1 : 0),
          k < n ? leafC : p.accent && dying <= 0 && L.y > 18 && L.y % 2 && h > 34 ? p.coin.d : tipC);
    }

    if (h > top || dying > 0) return; // dying plants drop their coin
    const tx = p.x + xo(h - 1), ty = GROUND - 1 - (h - 1);
    if (h < 10) px(tx, ty - 1, tipC);
    else if (h < 12) px(tx, ty - 1, p.coin.y);
    else drawTip(p, tx, ty, h);
  }

  // Thin, glowing tips in the coin's colour. Every plant starts as a short
  // spike; once tall enough it slowly develops its own form.
  const TIP_STYLES = ['spike', 'fork', 'berries', 'crown', 'cross']; // all symmetric
  function drawTip(p, tx, ty, h) {
    const c = p.coin;
    const glow = Math.sin(clock * 1.7 + p.phase) > 0.55 ? C.spark : c.h; // slow shimmer
    const len = Math.min(p.spikeMax, 2 + Math.floor((h - 12) / 7));
    const grad = [c.o, c.d, c.d, c.y, c.y, c.h];               // bottom -> top
    for (let k = 1; k <= len; k++)
      px(tx, ty - k, k === len ? glow : grad[Math.min(grad.length - 2, k - 1 + 6 - len)]);
    const top = ty - len;
    const m = h - 22;                                       // maturity
    if (m < 0) return;
    const grow = (step, max) => Math.min(max, 1 + Math.floor(m / step));

    switch (p.tip) {
      case 'fork': {                                        // two thin antennae
        const n = grow(8, 3);
        for (let k = 1; k <= n; k++) {
          const col = k === n ? c.h : c.y;
          px(tx - k, top - k, col); px(tx + k, top - k, col);
        }
        break;
      }
      case 'berries': {                                     // tiny dots around the tip
        const n = grow(7, p.dots.length);                   // mirrored pairs
        for (let i = 0; i < n; i++) {
          const [dx, dy] = p.dots[i], col = i % 2 ? c.h : c.y;
          px(tx - dx, top + dy, col); px(tx + dx, top + dy, col);
        }
        break;
      }
      case 'crown': {                                       // tiny tulip cup
        px(tx - 1, top + 1, c.d); px(tx + 1, top + 1, c.d);
        if (m > 8) { px(tx - 1, top, c.y); px(tx + 1, top, c.y); }
        if (m > 18) { px(tx - 2, top - 1, c.h); px(tx + 2, top - 1, c.h); }
        break;
      }
      case 'cross': {                                       // little arms, adding tiers
        const n = grow(9, 3);
        for (let k = 0; k < n; k++) {
          const yy = top + 1 + k * 2, col = k === 0 ? c.h : c.y;
          px(tx - 1, yy, col); px(tx + 1, yy, col);
        }
        break;
      }
    }
    // a rare single-pixel twinkle just above the tip
    if ((clock * 0.3 + p.phase) % 1 < 0.04) px(tx, top - 2, C.spark);
  }

  function drawHero() {
    const walking = phase === 'walkIn' || phase === 'walkOut';
    const crouch = phase === 'kneel' || (phase === 'drop' && phaseT < 0.25) ? 1 : 0;
    const hop = phase === 'cheer'
      ? -Math.round(Math.abs(Math.sin(phaseT / 0.8 * 2 * Math.PI)) * 3) : 0;
    const legF = walking ? Math.floor(clock * 8) % 4 : 0;
    const bob = walking && legF % 2 === 0 ? -1 : 0;
    const x = Math.round(hero.x), y = GROUND - 10 + crouch + hop + bob;

    const upper = hero.pose === 'cheer' ? CHEER : HEAD.concat(ARMS[hero.pose]);
    drawRows(upper.concat(LEGS[legF]), x, y, HERO_MAP, hero.dir < 0); // the sprite faces right; flipped, left
    if (hero.pose === 'carry') drawCoin(x + holdAt(), y + 3, SPIN[Math.floor(clock * 8) % 4], hero.coin);
  }

  function render() {
    ctx.clearRect(0, 0, W, H);
    clip = false;
    drawGround();
    plants.slice().sort((a, b) => a.depth - b.depth).forEach(drawPlant); // back to front
    clip = true;
    if (coin) {
      const f = coin.landed ? 0 : SPIN[Math.floor(clock * 14) % 4];
      drawCoin(coin.x, Math.round(coin.y + coin.sink), f, hero.coin);
    }
    drawHero();
    clip = false;
    for (const p of particles) px(p.x, p.y, C.dust);
  }

  // ---------- loop ----------
  hero = { x: EDGE[1], dir: -1 }; // from the right edge, facing left
  newCoin();
  startWalk();
  let last = performance.now(), raf;
  function frame(now) {
    const dt = Math.max(0, Math.min(0.1, (now - last) / 1000));
    fit();
    for (let i = 0; i < (opts.timeScale || 1); i++) update(dt);
    last = now;
    render();
    raf = requestAnimationFrame(frame);
  }
  function start() { cancelAnimationFrame(raf); last = performance.now(); raf = requestAnimationFrame(frame); }
  start();

  // Deltabadger: a scheme change recolours the scene in place.
  function recolor(colors) {
    Object.assign(C, colors);
    Object.assign(HERO_MAP, { w: C.suit, W: C.suitShade, v: C.visor, g: C.glint, a: C.accent, p: C.pack, B: C.boot });
    for (const p of plants) {
      p.stem = depthTone(C.stem, p.depth);
      p.leaf = depthTone(C.leaf, p.depth);
      p.leafTip = depthTone(C.leafL, p.depth);
    }
  }

  return {
    recolor,
    // Deltabadger: paused, the scene keeps its state and its clock stands still; attached again, it draws on.
    pause() { cancelAnimationFrame(raf); },
    attach(canvas) { setup(canvas); start(); },
    destroy() { cancelAnimationFrame(raf); },
  };
}

