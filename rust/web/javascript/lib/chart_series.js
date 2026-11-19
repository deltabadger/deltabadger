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
    fetched: false,
  };
}

// D-CHART-REV: an answer is drawn only if it answers the latest ask, and is not older than the data in hand.
export function keeps({ ask, latest, rev, held }) {
  return ask === latest && (held == null || rev >= held);
}

// The query of an ask: the whole history (548-1), the plot's room, and the extras that are on. Sorted, so equal asks
// are one key.
export function query({ points, asset, prices, buys }) {
  const params = new URLSearchParams();
  if (asset) params.set("asset", asset);
  if (buys) params.set("buys", "1");
  params.set("points", String(Math.min(Math.max(Math.floor(points) || 2, 2), 2000)));
  if (prices) params.set("prices", "1");
  return params.toString();
}

// The end dot sits on each dataset's own last point, whatever its length after its nulls were dropped.
export function endRadius(index, length) {
  return index === length - 1 ? 3 : 0;
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
