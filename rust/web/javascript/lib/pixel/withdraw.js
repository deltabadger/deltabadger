/**
 * "Automate withdrawals" empty-state animation — procedural pixel art.
 *
 * The hero sits on a treasure chest, swinging his legs. Every so often a
 * delivery drone flies in and drops a few coins nearby. The hero hops down,
 * picks each coin up, opens the chest, tosses it in, closes the lid and
 * climbs back on. Drop count, landing spots and coin colours are random.
 *
 * Usage:
 *   const anim = createWithdrawAnimation(canvasEl, { scale: 4, palette: 'mono', coins: 'stocks' });
 *   anim.destroy();
 */
export function createWithdrawAnimation(canvas, opts = {}) {
  const W = 160, H = 64, GROUND = H - 1;
  const SPEED = 24;
  const WAIT = opts.dropGap || [5, 9];           // seconds sitting between deliveries

  const PALETTES = {
    green: {},
    mono: { stem: '#2b4b7c', leaf: '#3d65a3', leafL: '#7397d2' },
  };
  const STOCK_HUES = [45, 140, 355, 268, 188, 24, 325, 215];

  const C = Object.assign({
    suit: '#e6edf9', suitShade: '#aebbd4', visor: '#1f3a6e', glint: '#9fd0ff', accent: '#3d63ff', pack: '#8a9bbd', boot: '#6c7fa8',
    coinO: '#c98a00', coinY: '#ffcc33', coinH: '#fff4b8', coinD: '#e8a600',
    stem: '#1f9d5a', leaf: '#2ecc71', leafL: '#8ef0b0',
    bg: '#011729', groundTop: '#1c3358', spark: '#ffffff',
    chestO: '#1c3358', chestW: '#2b4b7c', chestH: '#3d65a3', rivet: '#7397d2', panel: '#12284a', stripe: '#3d65a3',
    meterOff: '#22406b', meterOn: '#ffcc33', led: '#2ecc71', inside: '#071631',
    prop: '#cfe0ff', metal: '#7f95c4', metalDark: '#4f6796', lightOff: '#ff5a6a', lightOn: '#2ecc71',
    dog: '#8a9bbd', dogLight: '#b7c4dd', dogDark: '#4f6796', dogEye: '#ff5a6a',
  }, PALETTES[opts.palette] || {}, opts.colors || {});

  // ---------- sprites ----------
  const HEAD = ['..wwvv..', '.wwvvgv.', '.wwvvvv.', '..wwvv..']; // helmet with a glass bubble front
  const ARMS = {
    free:  ['..aaaa..', 'pwwwwww.', 'pWwwwwW.', '..wwww..'],
    carry: ['..aaaa..', 'pwwwwwWW', 'pwwwww..', '..wwww..'],
  };
  const LEGS = [
    ['..W..W..', '..B..B..'],
    ['..W.W...', '.B...B..'],
    ['..W..W..', '..B..B..'],
    ['...W.W..', '..B...B.'],
  ];
  const SIT = [
    '..wwvv...', '.wwvvgv..', '.wwvvvv..', '..wwvv...',
    '..aaaa...', 'pwwwwwW..', 'p.wwwwWW.',
  ];
  const SHINS = [
    ['......W..', '......W..', '......W..', '......B..'],
    ['.......W.', '.......W.', '........W', '........B'],
  ];
  const HERO_MAP = { w: C.suit, W: C.suitShade, v: C.visor, g: C.glint, a: C.accent, p: C.pack, B: C.boot };

  const COIN = [
    ['.ooo.', 'ohyyo', 'oyyyo', 'oyydo', '.ooo.'],
    ['.o.', 'oho', 'oyo', 'oyo', '.o.'],
    ['o', 'h', 'y', 'd', 'o'],
  ];
  const SPIN = [0, 1, 2, 1];

  const DRONE = [
    ['ppp.....ppp', '.m.......m.', '.mmmMMMmmm.', '....mLm....', '.....k.....'],
    ['.p.......p.', '.m.......m.', '.mmmMMMmmm.', '....mLm....', '.....k.....'],
  ];

  // space cargo container: brushed-metal shell, blue stripe, rivets, a dark
  // control panel with a fill meter and a status LED (drawn live in drawChest)
  const LID = ['.oooooooooooooo.', 'ohhhhhhhhhhhhhho', 'osssssssssssssso'];
  const BASE = [
    'oxwwwwwwwwwwwwxo',
    'owppppppppppppwo',
    'owppppppppppppwo',
    'owppppppppppppwo',
    'osssssssssssssso',
    'oxwwwwwwwwwwwwxo',
    'oooooooooooooooo',
  ];
  const CHEST_MAP = { o: C.chestO, w: C.chestW, h: C.chestH, x: C.rivet, p: C.panel, s: C.stripe };


  // ---------- canvas ----------
  const scale = opts.scale || 4;
  canvas.width = W; canvas.height = H;
  canvas.style.width = W * scale + 'px';
  canvas.style.height = H * scale + 'px';
  canvas.style.imageRendering = 'pixelated';
  const ctx = canvas.getContext('2d');

  function px(x, y, c) {
    x = Math.floor(x); y = Math.floor(y);
    if (x < 0 || x >= W || y < 0 || y >= H) return;
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
  function drawCoin(cx, top, frame, map) {
    const rows = COIN[frame];
    drawRows(rows, cx - (rows[0].length >> 1), top, map);
  }

  // ---------- rng + colours ----------
  function mulberry32(a) {
    return function () {
      a |= 0; a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }
  const rng = mulberry32(opts.seed ?? (Date.now() & 0xffffff));
  const rand = (a, b) => a + rng() * (b - a);

  const hex = c => [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16));
  const mix = (a, b, t) => '#' + hex(a).map((v, i) => Math.round(v + (hex(b)[i] - v) * t)
    .toString(16).padStart(2, '0')).join('');
  const depthTone = (c, d) => d < 0.5 ? mix(c, C.bg, (0.5 - d) * 1.2) : mix(c, '#ffffff', (d - 0.5) * 0.5);

  const GOLD = { o: C.coinO, y: C.coinY, h: C.coinH, d: C.coinD };
  const hueMap = hue => ({
    o: `hsl(${hue},70%,34%)`, y: `hsl(${hue},85%,58%)`,
    h: `hsl(${hue},100%,86%)`, d: `hsl(${hue},80%,47%)`,
  });
  const HUES = opts.coins === 'stocks' ? STOCK_HUES : Array.isArray(opts.coins) ? opts.coins : null;
  const COIN_MAPS = HUES ? HUES.map(hueMap) : [GOLD];

  const FADE = 16;
  function drawGround() {
    for (let x = 0; x < W; x++) {
      const e = Math.min(x, W - 1 - x);
      if (e >= FADE || ((x * 5) % FADE) < e) px(x, GROUND, C.groundTop);
    }
  }

  // ---------- chest ----------
  const CX = 80, CHX = CX - 8, CHY = GROUND - 10;  // chest left / top
  let lid = 0;          // 0 closed .. 1 open (animated)
  let lidTarget = 0, stored = 0;
  const glints = [];

  function drawChest() {
    drawRows(BASE, CHX, CHY + 3, CHEST_MAP);
    if (lid < 0.5) {
      drawRows(LID, CHX, CHY, CHEST_MAP);
    } else {
      // open: dark interior with the stash peeking through, lid raised
      for (let x = CHX + 1; x < CHX + 15; x++) { px(x, CHY + 2, C.inside); px(x, CHY + 1, C.chestO); }
      for (let i = 0; i < Math.min(stored, 14); i++) {
        const x = CHX + 1 + ((i * 5) % 14);
        px(x, CHY + 2, i % 3 ? C.coinD : C.coinY);
      }
      px(CHX, CHY + 1, C.chestO); px(CHX + 15, CHY + 1, C.chestO);
      px(CHX, CHY + 2, C.chestO); px(CHX + 15, CHY + 2, C.chestO);
      drawRows(LID, CHX, CHY - 3, CHEST_MAP);
    }
    // control panel: fill meter (grows with every coin stored) + blinking LED
    const SEGS = 8, lit = Math.min(SEGS, Math.ceil(stored / 2));
    for (let i = 0; i < SEGS; i++) px(CHX + 3 + i, CHY + 5, i < lit ? C.meterOn : C.meterOff);
    px(CHX + 12, CHY + 5, Math.sin(clock * 3) > 0.2 || lid > 0.5 ? C.led : C.panel);
  }

  // ---------- state ----------
  let clock = 0;
  const SEAT = { x: CHX - 2, y: CHY - 7 };
  const hero = { x: SEAT.x, y: SEAT.y, mode: 'sit', faceLeft: true, carry: null, t: 0, hop: null, queue: [] };
  const drone = { phase: 'away', t: 0, x: W + 2, y: 6, wait: 2.5, drops: [], carry: null };
  let coins = [];

  function planDelivery() {
    const n = 1 + Math.floor(rng() * 3);
    const left = rng() < 0.5;
    let x = left ? rand(52, 58) : rand(98, 104);   // close to the chest
    drone.drops = [];
    for (let i = 0; i < n; i++) {
      drone.drops.push(Math.round(x));
      x += (left ? -1 : 1) * rand(5, 7);
    }
    drone.phase = 'in'; drone.t = 0; drone.x = W + 2; drone.y = 4;
    drone.carry = COIN_MAPS[Math.floor(rng() * COIN_MAPS.length)];
  }

  function updateDrone(dt) {
    drone.t += dt;
    const bob = Math.sin(clock * 5) * 0.8;
    switch (drone.phase) {
      case 'away':
        if (hero.mode === 'sit' && drone.t > drone.wait) planDelivery();
        break;
      case 'in': case 'move': {
        const target = drone.drops[0] - 5;
        drone.x += (target - drone.x) * Math.min(1, dt * (drone.phase === 'in' ? 1.6 : 3));
        drone.y = 9 + bob;
        if (Math.abs(target - drone.x) < 0.6) { drone.phase = 'release'; drone.t = 0; }
        break;
      }
      case 'release':
        drone.y = 9 + bob;
        if (drone.t > 0.35) {
          const x = drone.drops.shift();
          coins.push({ x, y: drone.y + 5, vy: 0, map: drone.carry, state: 'fall', bounced: false });
          drone.carry = drone.drops.length ? COIN_MAPS[Math.floor(rng() * COIN_MAPS.length)] : null;
          drone.phase = drone.drops.length ? 'move' : 'out'; drone.t = 0;
        }
        break;
      case 'out':
        drone.x -= dt * 40; drone.y -= dt * 8;
        if (drone.x < -14) {
          drone.phase = 'away'; drone.t = 0; drone.wait = rand(WAIT[0], WAIT[1]);
        }
        break;
    }
  }

  function updateCoins(dt) {
    for (const c of coins) {
      if (c.state !== 'fall') continue;
      c.vy += 260 * dt; c.y += c.vy * dt;
      if (c.y + 5 >= GROUND) {
        c.y = GROUND - 5;
        if (!c.bounced && c.vy > 40) { c.vy = -c.vy * 0.3; c.bounced = true; }
        else { c.state = 'rest'; c.vy = 0; }
      }
    }
  }

  // hero helpers
  const STAND_Y = GROUND - 10;
  const side = () => (coins.length && coins[0].x < CX ? 'left' : 'right');
  const depositX = s => (s === 'left' ? CHX - 12 : CHX + 19);
  function hopTo(x, y, then) {
    hero.hop = { x0: hero.x, y0: hero.y, x1: x, y1: y, t: 0, then };
    hero.mode = 'hop';
  }
  function walkTo(x, then) { hero.mode = 'walk'; hero.target = x; hero.then = then; }

  function nextCoin() {
    const c = coins.find(c => c.state === 'rest');
    if (!c) {
      lidTarget = 0;
      hero.mode = 'wait'; hero.t = 0;
      hero.then = () => hopTo(SEAT.x, SEAT.y, () => { hero.mode = 'sit'; hero.faceLeft = true; });
      return;
    }
    c.state = 'claimed';
    const faceLeft = c.x < hero.x;
    walkTo(faceLeft ? c.x + 3 : c.x - 10, () => {
      hero.faceLeft = faceLeft;
      hero.mode = 'pick'; hero.t = 0;
      hero.then = () => {
        hero.carry = c; c.state = 'carried';
        const s = c.x < CX ? 'left' : 'right';
        walkTo(depositX(s), () => {
          hero.faceLeft = s === 'right';
          lidTarget = 1;
          hero.mode = 'open'; hero.t = 0;
          hero.then = () => {
            const hx = hero.faceLeft ? hero.x - 3 : hero.x + 10;
            c.state = 'toss'; c.toss = { x0: hx, y0: hero.y + 3, t: 0 };
            hero.carry = null;
            hero.mode = 'wait'; hero.t = 0;
            hero.then = nextCoin;
          };
        });
      };
    });
  }

  function updateHero(dt) {
    hero.t += dt;
    switch (hero.mode) {
      case 'sit':
        if (coins.length && coins.every(c => c.state === 'rest') && (drone.phase === 'out' || drone.phase === 'away')) {
          const s = side();
          hopTo(depositX(s), STAND_Y, nextCoin);
        }
        break;
      case 'hop': {
        const h = hero.hop;
        h.t += dt / 0.45;
        const t = Math.min(1, h.t);
        hero.x = h.x0 + (h.x1 - h.x0) * t;
        hero.y = h.y0 + (h.y1 - h.y0) * t - Math.sin(t * Math.PI) * 6;
        hero.faceLeft = h.x1 < h.x0 || (h.x1 === h.x0 && hero.faceLeft);
        if (t >= 1) { hero.y = h.y1; hero.hop = null; h.then(); }
        break;
      }
      case 'walk': {
        const d = hero.target - hero.x;
        hero.faceLeft = d < 0;
        if (Math.abs(d) <= SPEED * dt) { hero.x = hero.target; hero.then(); }
        else hero.x += Math.sign(d) * SPEED * dt;
        break;
      }
      case 'pick': if (hero.t > 0.35) hero.then(); break;
      case 'open': if (hero.t > 0.3) hero.then(); break;
      case 'wait': if (hero.t > 0.55) hero.then(); break;
    }
  }

  function updateToss(dt) {
    for (const c of coins) {
      if (c.state !== 'toss') continue;
      c.toss.t += dt / 0.4;
      const t = Math.min(1, c.toss.t);
      c.x = c.toss.x0 + (CX - c.toss.x0) * t;
      c.y = c.toss.y0 + (CHY - 1 - c.toss.y0) * t - Math.sin(t * Math.PI) * 8;
      if (t >= 1) {
        c.state = 'done'; stored++;
        for (let i = 0; i < 6; i++)
          glints.push({ x: CX + rand(-4, 4), y: CHY + 1, vx: rand(-14, 14), vy: rand(-40, -20), life: 0.6, map: c.map });
      }
    }
    coins = coins.filter(c => c.state !== 'done');
    for (const g of glints) { g.vy += 90 * dt; g.x += g.vx * dt; g.y += g.vy * dt; g.life -= dt; }
    for (let i = glints.length - 1; i >= 0; i--) if (glints[i].life <= 0) glints.splice(i, 1);
  }

  function update(dt) {
    clock += dt;
    updateDrone(dt);
    updateCoins(dt);
    updateHero(dt);
    updateToss(dt);
    lid += Math.sign(lidTarget - lid) * Math.min(Math.abs(lidTarget - lid), dt * 5);
    updateDog(dt);
  }

  // ---------- robot dog: sneaks up on fallen coins, bolts once they're taken ----------
  const DOG_CHANCE = opts.dogChance ?? 0.8;
  const dog = { state: 'off', x: 0, dir: 1, moving: false, t: 0, rolled: false };
  const dogTargets = () => coins.filter(c => c.state === 'rest' || c.state === 'claimed');

  function updateDog(dt) {
    dog.t += dt;
    const targets = dogTargets();
    switch (dog.state) {
      case 'off':
        if (!targets.length) { dog.rolled = false; break; }
        if (dog.rolled || !coins.every(c => c.state !== 'fall')) break;
        dog.rolled = true;
        if (rng() > DOG_CHANCE) break;
        dog.dir = targets[0].x < CX ? 1 : -1;            // enter from the coins' side
        dog.x = dog.dir > 0 ? -12 : W + 2;
        dog.state = 'creep'; dog.t = 0;
        break;
      case 'creep': {
        if (!targets.length) { dog.state = 'flee'; dog.t = 0; dog.dir = -dog.dir; break; }
        const front = dog.dir > 0 ? dog.x + 14 : dog.x - 3;
        const c = targets.reduce((a, b) => Math.abs(b.x - front) < Math.abs(a.x - front) ? b : a);
        const gap = dog.dir > 0 ? c.x - 3 - front : front - (c.x + 3);
        // creep in little stop-start steps
        dog.moving = gap > 0 && Math.sin(dog.t * 5) > -0.2;
        if (dog.moving) dog.x += dog.dir * Math.min(gap, 9 * dt);
        break;
      }
      case 'flee':
        dog.moving = true;
        if (dog.t > 0.35) dog.x += dog.dir * 70 * dt;      // brief "!" freeze, then bolt
        if (dog.x < -14 || dog.x > W + 4) { dog.state = 'off'; dog.rolled = targets.length > 0; }
        break;
    }
  }

  function line(x0, y0, x1, y1, c) {
    const n = Math.max(Math.abs(x1 - x0), Math.abs(y1 - y0), 1);
    for (let i = 0; i <= n; i++) px(Math.round(x0 + (x1 - x0) * i / n), Math.round(y0 + (y1 - y0) * i / n), c);
  }

  // spider-like quadruped: boxy chassis on four jointed legs with knees raised
  // above the body; legs step in a procedural diagonal gait
  const DOG_LEGS = [   // hip x, outward direction, far side?, gait phase
    { hx: 3, out: -1, far: true,  ph: 0 },
    { hx: 8, out: 1,  far: true,  ph: Math.PI },
    { hx: 4, out: -1, far: false, ph: Math.PI },
    { hx: 7, out: 1,  far: false, ph: 0 },
  ];
  function drawDog() {
    if (dog.state === 'off') return;
    const fleeing = dog.state === 'flee';
    const x = Math.round(dog.x);
    const X = lx => (dog.dir > 0 ? x + lx : x + 11 - lx);
    const by = GROUND - 8 + (dog.state === 'creep' ? 1 : 0);   // chassis top
    const speed = fleeing ? 22 : 9;

    const leg = L => {
      const a = dog.t * speed + L.ph;
      const stride = dog.moving ? Math.round(Math.sin(a) * 1.5) : 0;
      const lift = dog.moving && Math.cos(a) > 0.3 ? 1 : 0;
      const kx = L.hx + L.out * 3, ky = by - 2 + (L.far ? 1 : 0);
      const fx = L.hx + L.out * 6 + stride, fy = GROUND - 1 - lift;
      const col = L.far ? C.dogDark : C.dog;
      line(X(L.hx), by + 2, X(kx), ky, col);
      line(X(kx), ky, X(fx), fy, col);
    };
    DOG_LEGS.filter(L => L.far).forEach(leg);

    for (let lx = 2; lx <= 8; lx++) {                          // chassis
      px(X(lx), by, C.dogLight); px(X(lx), by + 1, C.dog); px(X(lx), by + 2, C.dogDark);
    }
    px(X(9), by, C.dog); px(X(10), by, C.dog);                 // sensor head
    px(X(9), by + 1, C.dogDark);
    px(X(10), by + 1, Math.sin(clock * 7) > -0.6 ? C.dogEye : C.dogDark);
    px(X(4), by - 1, C.dogDark);                               // antenna
    px(X(4), by - 2, Math.sin(clock * 3) > 0 ? C.dogEye : C.dogDark);

    DOG_LEGS.filter(L => !L.far).forEach(leg);

    if (fleeing && dog.t < 0.6) {                              // startled "!"
      const ex = X(9);
      px(ex, by - 6, C.spark); px(ex, by - 5, C.spark); px(ex, by - 3, C.spark);
    }
  }


  // ---------- drawing ----------
  function drawHero() {
    const x = Math.round(hero.x);
    if (hero.mode === 'sit') {
      const lookUp = drone.phase !== 'away';
      const swing = lookUp ? 0 : Math.floor(clock * 2.5) % 2;
      const rows = SIT.concat(SHINS[swing]);
      drawRows(rows, x, Math.round(hero.y), HERO_MAP, true);
      return;
    }
    const walking = hero.mode === 'walk';
    const legF = walking ? Math.floor(clock * 8) % 4 : 0;
    const bob = walking && legF % 2 === 0 ? -1 : 0;
    const crouch = hero.mode === 'pick' ? 1 : 0;
    const y = Math.round(hero.y) + bob + crouch;
    const arms = hero.carry || hero.mode === 'pick' || hero.mode === 'open' ? ARMS.carry : ARMS.free;
    drawRows(HEAD.concat(arms, LEGS[legF]), x, y, HERO_MAP, hero.faceLeft);
    if (hero.carry)
      drawCoin(hero.faceLeft ? x - 3 : x + 10, y + 3, SPIN[Math.floor(clock * 8) % 4], hero.carry.map);
  }

  function drawDrone() {
    if (drone.phase === 'away') return;
    const x = Math.round(drone.x), y = Math.round(drone.y);
    const lit = drone.phase === 'release' ? C.lightOn : Math.sin(clock * 6) > 0 ? C.lightOff : C.metalDark;
    drawRows(DRONE[Math.floor(clock * 20) % 2], x, y,
      { p: C.prop, m: C.metal, M: C.metalDark, L: lit, k: C.metalDark });
    if (drone.carry) drawCoin(x + 5, y + 5, SPIN[Math.floor(clock * 6) % 4], drone.carry);
  }

  function render() {
    ctx.clearRect(0, 0, W, H);
    drawGround();
    for (const c of coins) {
      if (c.state === 'carried') continue;
      const f = c.state === 'rest' || c.state === 'claimed' ? 0 : SPIN[Math.floor(clock * 14) % 4];
      drawCoin(Math.round(c.x), Math.round(c.y), f, c.map);
      if ((c.state === 'rest' || c.state === 'claimed') && (clock * 0.7 + c.x * 0.13) % 1 < 0.06) px(c.x + 2, c.y - 1, C.spark);
    }
    drawChest();
    drawDog();
    drawHero();
    for (const c of coins) if (c.state === 'toss') drawCoin(Math.round(c.x), Math.round(c.y), SPIN[Math.floor(clock * 14) % 4], c.map);
    for (const g of glints) px(g.x, g.y, g.life > 0.3 ? g.map.h : g.map.y);
    drawDrone();
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
    Object.assign(CHEST_MAP, { o: C.chestO, w: C.chestW, h: C.chestH, x: C.rivet, p: C.panel, s: C.stripe });
  }

  return { recolor, destroy() { cancelAnimationFrame(raf); } };
}

