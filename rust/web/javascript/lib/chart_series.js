// What the bot chart draws from, read from either source into ONE shape (D-CHART-DATA-REQUEST): the answer of
// `GET /bots/{id}/chart/data`, or the attributes the tracker still writes. Every series runs the length of `labels`,
// one index one moment. Pure, so `bun test` checks it (chart_series.test.js).

const time = (at) => (typeof at === "number" ? at : new Date(at).getTime());

// One ruler across the holdings, over every point: what the tracker's attributes carry is all of them.
export function assetBounds(assets) {
  return Object.values(assets).reduce(
    (span, asset) => {
      asset.value.forEach((amount, i) => {
        const cost = asset.invested[i];
        // The invested line is drawn at every point, including ones where value is unmarked.
        span.value = Math.max(span.value, cost);
        if (amount === null) return;

        span.value = Math.max(span.value, amount);
        span.low = Math.min(span.low, amount - cost);
        span.high = Math.max(span.high, amount - cost);
      });
      return span;
    },
    // Seeded at zero, which is where both modes read from: VALUE's floor and PnL's baseline.
    { value: 0, low: 0, high: 0 }
  );
}

// The server's answer. `assets` holds the one holding asked for, `prices` and `buys` only when asked: what the
// answer does not carry, the chart is not drawing. `holdings`, `priced` and `bounds` speak for the whole history.
export function fromAnswer(answer) {
  const value = answer.value;
  const invested = answer.invested;
  return {
    rev: answer.rev,
    labels: answer.t,
    series: [value, invested],
    // value minus invested, as the attribute was: the same subtraction off the same rounded series.
    pnl: value.map((amount, i) => amount - invested[i]),
    assets: answer.asset ? { [answer.asset.key]: { value: answer.asset.value, invested: answer.asset.invested } } : {},
    asset: answer.asset?.key ?? null,
    prices: answer.prices ?? null,
    buys: answer.buys ?? null,
    logos: answer.logos ?? {},
    holdings: answer.holdings ?? [],
    priced: answer.priced ?? [],
    bounds: answer.bounds ?? { value: 0, low: 0, high: 0 },
    extent: answer.extent,
    // 548-2: what the answer covers. `spans` are the view widths its step serves, `padded` the times its selection
    // reaches ([since, until)); `closed` and `sum` are what the next ask repeats for an append (D-CHART-LIVE).
    step: answer.step,
    spans: answer.spans,
    padded: answer.padded,
    closed: answer.closed ?? null,
    sum: answer.sum ?? null,
    fetched: true,
  };
}

// The tracker's attributes (and any chart without `src`): everything is here, so the lists are read off it.
export function fromAttributes({ labels = [], series = [[], []], pnl = [], assets = {}, buys = [], prices = {}, buyLogos = {} }) {
  return {
    rev: null,
    labels: labels.map(time),
    series,
    pnl,
    assets,
    asset: null,
    prices,
    buys: buys.map(([at, ...rest]) => [time(at), ...rest]),
    logos: buyLogos,
    holdings: Object.keys(assets).filter((key) => assets[key].value?.some((amount) => amount !== null)),
    priced: Object.keys(prices),
    bounds: assetBounds(assets),
    extent: [time(labels[0]), time(labels.at(-1))],
    step: null,
    spans: null,
    padded: null,
    closed: null,
    sum: null,
    fetched: false,
  };
}

// D-CHART-REV: an answer is drawn only if it answers the latest ask, and is not older than the data in hand.
export function keeps({ ask, latest, rev, held }) {
  return ask === latest && (held == null || rev >= held);
}

// The query of an ask: the view in hand (none: the whole history), the plot's room, and the extras that are on. Sorted,
// so equal asks are one key. `after` and `sum` are not part of it: they ride on a key.
export function query({ points, asset, prices, buys, view = null }) {
  const params = new URLSearchParams();
  if (asset) params.set("asset", asset);
  if (buys) params.set("buys", "1");
  if (view) params.set("from", String(Math.floor(view.min)));
  params.set("points", String(Math.min(Math.max(Math.floor(points) || 2, 2), 2000)));
  if (prices) params.set("prices", "1");
  if (view) params.set("to", String(Math.ceil(view.max)));
  return params.toString();
}

// D-CHART-LIVE: an answer with `"append": true` to an ask with `after`: the data in hand before the ASK's `after` (not
// the answer's `closed`, which can be later), then the answer's points. Everything else is the answer's own.
export function append(held, answer, after) {
  const keep = held.t.findIndex((at) => at >= after);
  const k = keep < 0 ? held.t.length : keep;
  const join = (before, from) => before.slice(0, k).concat(from);
  const merged = { ...answer, append: false, t: join(held.t, answer.t), value: join(held.value, answer.value), invested: join(held.invested, answer.invested) };
  if (answer.asset) merged.asset = { ...answer.asset, value: join(held.asset.value, answer.asset.value), invested: join(held.asset.invested, answer.asset.invested) };
  if (answer.prices) merged.prices = Object.fromEntries(Object.entries(answer.prices).map(([key, serie]) => [key, join(held.prices[key], serie)]));
  return merged;
}

// The whole-history answers of one revision, by query: "everything" is drawn from hand while its `rev` and its extras
// are the ones on screen. A narrowed view's answer is only ever the data in hand.
export function remember(kept, key, answer) {
  if (!(answer.rev <= kept.rev)) {
    kept.answers.clear();
    kept.rev = answer.rev;
  }
  if (answer.rev === kept.rev && !new URLSearchParams(key).has("from")) kept.answers.set(key, answer);
}

// The points of `labels` inside [min, max]: [first, end).
export function inView(labels, { min, max }) {
  const bound = (at, strict) => {
    let low = 0;
    let high = labels.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (strict ? labels[middle] <= at : labels[middle] < at) low = middle + 1;
      else high = middle;
    }
    return low;
  };
  return [bound(min, false), bound(max, true)];
}

// What a view's y range and price lines are measured on: its points and the neighbour either side, so the frame holds
// the line where it crosses the plot's edges. Padding past those takes no part. As times, [min, max].
export function measured(labels, view) {
  const [first, end] = inView(labels, view);
  return { min: labels[Math.max(first - 1, 0)], max: labels[Math.min(end, labels.length - 1)] };
}

// D-CHART-VIEW A: a narrowed view reads its own period. Its P/L runs from the first point in view (with a reading) to
// the last, and that first reading is the baseline the curve and the headline subtract; the whole history reads from 0.
export function reading(labels, serie, view, narrowed) {
  const [first, end] = inView(labels, view);
  let last = end - 1;
  while (last >= first && serie[last] == null) last -= 1;
  let opening = first;
  while (opening < end && serie[opening] == null) opening += 1;
  const baseline = narrowed && opening < end ? Number(serie[opening]) : 0;
  return { first, end, last: last >= first ? last : -1, baseline };
}

// 548-2: the data in hand no longer fit the view when its width has left their `spans`, or it comes within a quarter
// of its width of the edge of what they cover while the history goes on beyond it.
export function asks({ view, spans, padded, extent }) {
  if (!spans || !padded) return false;
  const width = view.max - view.min;
  if (width < spans[0] || width > spans[1]) return true;
  const margin = width / 4;
  return (view.min - padded[0] < margin && padded[0] > extent[0]) || (padded[1] - view.max < margin && padded[1] <= extent[1]);
}

// The view a gesture asks for, kept inside the history and no narrower than `least`: null is the whole history.
export function clampView(min, max, [low, high], least) {
  const full = high - low;
  const span = Math.min(Math.max(max - min, least), full);
  if (!(full > 0) || span >= full) return null;
  const from = Math.min(Math.max(min, low), high - span);
  return { min: from, max: from + span };
}

// The hovered point: the nearest of `positions` to `x` among those inside [low, high] (the plot's pixels), -1 when
// none is: a padding point never reaches the headline.
export function nearest(positions, x, low, high) {
  let index = -1;
  let distance = Infinity;
  positions.forEach((at, i) => {
    if (at < low - 0.5 || at > high + 0.5) return;
    if (Math.abs(at - x) < distance) {
      distance = Math.abs(at - x);
      index = i;
    }
  });
  return index;
}

// The end dot: on a dataset's own last point, and only while that point is in view.
export function endRadius(index, length, at = null, view = null) {
  if (index !== length - 1) return 0;
  return !view || at == null || (at >= view.min && at <= view.max) ? 3 : 0;
}

// The buys drawn: on the holding in view, inside [from, to] by their own times.
export function buysIn(buys, { from, to, focused }) {
  return (buys || []).filter(([at, symbol]) => (!focused || symbol === focused) && at >= from && at <= to);
}

// A series as points to draw, nulls dropped (every point carries its own time, so a gap costs nothing), `shift` added.
export function curve(labels, serie, shift = 0) {
  return serie
    .map((amount, i) => ({ x: labels[i], y: amount === null ? null : Number(amount) + shift }))
    .filter((point) => point.y !== null);
}

// --- the dates under the plot -------------------------------------------------------
//
// The only axis this chart has: a handful of round dates under the curve, at the coarsest unit
// the window in view holds `AXIS_MIN` boundaries of, thinned by the first step that leaves each
// label `AXIS_ROOM` px to print in. A boundary that OPENS the unit above is written as that unit
// instead — January says "2026", midnight says the date — so a row of months carries its year
// without spending a label on it.
//
// Coarsest first. The steps are the ones that keep the run aligned to the bigger unit: months
// step by a divisor of 12 so a January is always among them, hours likewise by a divisor of 24,
// minutes and seconds by a divisor of 60. Days cannot be — a month is not a round number of them —
// which is why a day reads as "14 Jul" and carries its own month.
const AXIS_MIN = 4;
const AXIS_ROOM = 80;
const DAY = { day: "numeric", month: "short" };
const MINUTE = { hour: "numeric", minute: "2-digit" };
export const AXIS_UNITS = [
  {
    size: 31536e6, steps: [1, 2, 5, 10, 25],
    label: { year: "numeric" },
    start: (at, step) => new Date(at.getFullYear() - (at.getFullYear() % step), 0),
    next: (at, step) => at.setFullYear(at.getFullYear() + step),
  },
  {
    size: 2592e6, steps: [1, 2, 3, 4, 6, 12],
    label: { month: "short" }, opens: (at) => at.getMonth() === 0, above: { year: "numeric" },
    start: (at, step) => new Date(at.getFullYear(), at.getMonth() - (at.getMonth() % step)),
    next: (at, step) => at.setMonth(at.getMonth() + step),
  },
  {
    size: 864e5, steps: [1, 2, 5, 10, 15],
    label: DAY,
    start: (at) => new Date(at.getFullYear(), at.getMonth(), at.getDate()),
    next: (at, step) => at.setDate(at.getDate() + step),
  },
  {
    size: 36e5, steps: [1, 2, 3, 4, 6, 12],
    label: MINUTE, opens: (at) => at.getHours() === 0, above: DAY,
    start: (at, step) => new Date(at.getFullYear(), at.getMonth(), at.getDate(),
                                 at.getHours() - (at.getHours() % step)),
    next: (at, step) => at.setHours(at.getHours() + step),
  },
  {
    // An hour opening is written as the hour unit writes it, which is the same "10:00"; midnight as the date.
    size: 6e4, steps: [1, 2, 5, 10, 15, 30],
    label: MINUTE, opens: (at) => at.getHours() === 0 && at.getMinutes() === 0, above: DAY,
    start: (at, step) => new Date(at.getFullYear(), at.getMonth(), at.getDate(), at.getHours(),
                                 at.getMinutes() - (at.getMinutes() % step)),
    next: (at, step) => at.setMinutes(at.getMinutes() + step),
  },
  {
    size: 1e3, steps: [1, 2, 5, 10, 15, 30],
    label: { ...MINUTE, second: "2-digit" }, opens: (at) => at.getSeconds() === 0, above: MINUTE,
    start: (at, step) => new Date(at.getFullYear(), at.getMonth(), at.getDate(), at.getHours(), at.getMinutes(),
                                 at.getSeconds() - (at.getSeconds() % step)),
    next: (at, step) => at.setSeconds(at.getSeconds() + step),
  },
];

// The labelled boundaries of [min, max] on a plot `width` px wide: [{ at, format }], `format` for toLocaleString.
// Walked with the calendar's own arithmetic in the reader's zone, not by adding a nominal length: a month is not 30
// days and a DST day is not 24 hours, and either would drift the boundaries off the dates they are named after.
export function axisMarks(min, max, width) {
  const span = max - min;
  const unit = AXIS_UNITS.find(({ size }) => span / size >= AXIS_MIN) || AXIS_UNITS.at(-1);
  const room = Math.max(2, Math.floor(width / AXIS_ROOM));
  const step = unit.steps.find((jump) => span / unit.size / jump <= room) || unit.steps.at(-1);
  const marks = [];
  const at = unit.start(new Date(min), step);
  while (+at <= max) {
    if (+at >= min) marks.push({ at: +at, format: unit.opens?.(at) ? unit.above : unit.label });
    unit.next(at, step);
  }
  return marks;
}
