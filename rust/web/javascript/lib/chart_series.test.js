// bun test rust/web/javascript/lib — the bot chart's reading of its data (chart_series.js).
import { describe, expect, test } from "bun:test";
import {
  append, asks, axisMarks, buysIn, clampView, curve, endRadius, fromAnswer, fromAttributes, keeps, measured, nearest, query,
  reading, remember,
} from "./chart_series.js";

const answer = (extra = {}) => ({
  rev: 41, quote: "USDT", decimals: 2, step: 86400, spans: [0, 1], extent: [1000, 5000],
  t: [1000, 3000, 5000], value: [10, 12, 15], invested: [10, 10, 11],
  holdings: ["BTC"], priced: ["BTC"], logos: { BTC: { image: null, color: "#F7931A" } },
  bounds: { value: 9, low: -1, high: 2 },
  ...extra,
});

describe("an answer", () => {
  test("every series runs the length of t", () => {
    const model = fromAnswer(answer({
      asset: { key: "BTC", value: [null, 5, 6], invested: [0, 4, 4] },
      prices: { BTC: [100, null, 120] },
    }));
    const n = model.labels.length;
    expect(n).toBe(3);
    for (const serie of [...model.series, model.pnl, model.assets.BTC.value, model.assets.BTC.invested, model.prices.BTC]) {
      expect(serie.length).toBe(n);
    }
    expect(model.pnl).toEqual([0, 2, 4]);
    expect(model.asset).toBe("BTC");
  });

  test("one point", () => {
    const model = fromAnswer(answer({ t: [5000], value: [15], invested: [11], extent: [5000, 5000] }));
    expect(model.labels).toEqual([5000]);
    expect(model.pnl).toEqual([4]);
    expect(curve(model.labels, model.series[0])).toEqual([{ x: 5000, y: 15 }]);
  });

  test("what it was not asked for, it does not carry", () => {
    const model = fromAnswer(answer());
    expect(model.assets).toEqual({});
    expect(model.asset).toBeNull();
    expect(model.prices).toBeNull();
    expect(model.buys).toBeNull();
    expect(model.holdings).toEqual(["BTC"]);
  });
});

test("the tracker's attributes read to the same shape", () => {
  const model = fromAttributes({
    labels: ["1970-01-01T00:00:01.000Z", "1970-01-01T00:00:03.000Z"],
    series: [["10.0", "12.0"], ["10.0", "10.0"]],
    pnl: [0, 2],
  });
  const fetched = fromAnswer(answer({ t: [1000, 3000], value: [10, 12], invested: [10, 10], extent: [1000, 3000] }));
  expect(Object.keys(model).sort()).toEqual(Object.keys(fetched).sort());
  expect(model.labels).toEqual([1000, 3000]);
  expect(model.extent).toEqual([1000, 3000]);
  expect(model.pnl).toEqual(fetched.pnl);
  expect(model.holdings).toEqual([]);
  expect(model.fetched).toBe(false);

  // A bot chart with all its series in attributes reads its holdings and ruler off them.
  const assets = { BTC: { value: [null, 5], invested: [0, 4] }, ETH: { value: [null, null], invested: [1, 1] } };
  const full = fromAttributes({ labels: [1000, 3000], series: [[1, 2], [1, 1]], pnl: [0, 1], assets, buys: [["1970-01-01T00:00:03.000Z", "BTC", 1, 2, 1]] });
  expect(full.holdings).toEqual(["BTC"]);
  expect(full.bounds).toEqual({ value: 5, low: 0, high: 1 });
  expect(full.buys).toEqual([[3000, "BTC", 1, 2, 1]]);
});

describe("an arriving answer (D-CHART-REV)", () => {
  test("a late answer to an older ask is dropped", () => {
    expect(keeps({ ask: 3, latest: 4, rev: 41, held: 41 })).toBe(false);
    expect(keeps({ ask: 4, latest: 4, rev: 41, held: 41 })).toBe(true);
  });

  test("an answer older than the data in hand is dropped", () => {
    expect(keeps({ ask: 4, latest: 4, rev: 40, held: 41 })).toBe(false);
    expect(keeps({ ask: 4, latest: 4, rev: 42, held: 41 })).toBe(true);
    expect(keeps({ ask: 1, latest: 1, rev: 40, held: undefined })).toBe(true);
  });
});

test("the buys are kept by the history's ends, not by the first point in hand", () => {
  // The selected points start at 3000; two buys sit before it, one exactly on the first point of the history.
  const buys = [[1000, "BTC", 1, 10, 1], [2000, "ETH", 1, 10, 1], [5000, "BTC", 1, 10, 1], [6000, "BTC", 1, 10, 1]];
  const model = fromAnswer(answer({ t: [3000, 5000], value: [1, 2], invested: [1, 1], buys }));
  const [from, to] = model.extent;
  expect(buysIn(model.buys, { from, to, focused: null }).map(([at]) => at)).toEqual([1000, 2000, 5000]);
  expect(buysIn(model.buys, { from, to, focused: "BTC" }).map(([at]) => at)).toEqual([1000, 5000]);
  expect(buysIn(null, { from, to, focused: null })).toEqual([]);
});

test("a holding all null in the answer draws nothing, in P/L and in VALUE", () => {
  const model = fromAnswer(answer({ asset: { key: "BTC", value: [null, null, null], invested: [0, 0, 5] } }));
  const { value, invested } = model.assets.BTC;
  // VALUE: the value curve has no point, the invested line all of them.
  expect(curve(model.labels, value)).toEqual([]);
  expect(curve(model.labels, invested).length).toBe(3);
  // P/L: value minus invested, null where the value is.
  const pnl = value.map((amount, i) => (amount === null ? null : amount - invested[i]));
  expect(curve(model.labels, pnl)).toEqual([]);
});

test("the end dot sits on each dataset's own last point", () => {
  // A value curve of 2 points after its nulls were dropped, beside an invested line of 3.
  const dots = (length) => Array.from({ length }, (_, i) => endRadius(i, length));
  expect(dots(2)).toEqual([0, 3]);
  expect(dots(3)).toEqual([0, 0, 3]);
  expect(dots(1)).toEqual([3]);
});

test("equal asks are one query", () => {
  expect(query({ points: 300.7, asset: "BTC", prices: true, buys: false })).toBe("asset=BTC&points=300&prices=1");
  expect(query({ points: 1, asset: null, prices: false, buys: true })).toBe("buys=1&points=2");
  expect(query({ points: 9000, asset: null, prices: false, buys: false })).toBe("points=2000");
});

// --- 548-2: the range in view -------------------------------------------------------------------------------------

describe("the view reading (D-CHART-VIEW A)", () => {
  const labels = [0, 10, 20, 30, 40, 50];
  const pnl = [1, 2, null, 5, 7, 9];

  test("a narrowed view reads from its first point with a reading to its last", () => {
    // The view [15, 45] holds 20 (no reading), 30, 40; 10 and 50 are the padding either side.
    expect(reading(labels, pnl, { min: 15, max: 45 }, true)).toEqual({ first: 2, end: 5, last: 4, baseline: 5 });
  });

  test("the whole history reads from zero", () => {
    expect(reading(labels, pnl, { min: 0, max: 50 }, false)).toEqual({ first: 0, end: 6, last: 5, baseline: 0 });
  });

  test("a view with no point in it has no last point", () => {
    expect(reading(labels, pnl, { min: 41, max: 49 }, true).last).toBe(-1);
  });
});

test("marks and extremes in the padding take no part", () => {
  const labels = [0, 10, 20, 30, 40, 50];
  // In view: 20 and 30; their neighbours 10 and 40 measure the edges; 0 and 50 are padding.
  expect(measured(labels, { min: 15, max: 35 })).toEqual({ min: 10, max: 40 });
  const value = [999, 1, 2, 3, 4, -999];
  const points = curve(labels, value);
  const { min, max } = measured(labels, { min: 15, max: 35 });
  const inside = points.filter((point) => point.x >= min && point.x <= max).map((point) => point.y);
  expect(Math.max(...inside)).toBe(4);
  expect(Math.min(...inside)).toBe(1);
  // The marks are kept by the scale's bounds: the view, not the points in hand.
  const buys = [[5, "BTC", 1, 1, 1], [25, "BTC", 1, 1, 1], [45, "BTC", 1, 1, 1]];
  expect(buysIn(buys, { from: 15, to: 35, focused: null }).map(([at]) => at)).toEqual([25]);
});

describe("the hover", () => {
  test("at the plot's edge it never picks a padding point", () => {
    // Pixels: a padding point 5 px left of the plot, one just inside, the last inside, one past the right edge.
    const positions = [-5, 0.2, 50, 100, 104];
    expect(nearest(positions, -3, 0, 100)).toBe(1);
    expect(nearest(positions, 103, 0, 100)).toBe(3);
  });

  test("a view with no selected point has none", () => {
    expect(nearest([-10, 110], 50, 0, 100)).toBe(-1);
  });
});

describe("an append (D-CHART-LIVE)", () => {
  const held = {
    rev: 41, t: [0, 10, 20, 30], value: [1, 2, 3, 4], invested: [1, 1, 1, 1], closed: 20, sum: "a",
    asset: { key: "BTC", value: [null, 1, 2, 3], invested: [0, 1, 1, 1] }, prices: { BTC: [5, 6, 7, 8] },
    buys: [[10, "BTC", 1, 1, 1]],
  };

  test("keeps the data in hand up to the ask's after, even when the answer's closed is later", () => {
    const answer = {
      rev: 42, append: true, t: [20, 30, 40], value: [3, 5, 6], invested: [1, 1, 2], closed: 40, sum: "b",
      asset: { key: "BTC", value: [2, 4, 5], invested: [1, 1, 2] }, prices: { BTC: [7, 9, 10] },
      buys: [[10, "BTC", 1, 1, 1], [40, "BTC", 1, 1, 1]],
    };
    const merged = append(held, answer, 20);
    expect(merged.t).toEqual([0, 10, 20, 30, 40]);
    expect(merged.value).toEqual([1, 2, 3, 5, 6]);
    expect(merged.invested).toEqual([1, 1, 1, 1, 2]);
    expect(merged.asset.value).toEqual([null, 1, 2, 4, 5]);
    expect(merged.prices.BTC).toEqual([5, 6, 7, 9, 10]);
    expect(merged.buys).toEqual(answer.buys);
    expect([merged.rev, merged.closed, merged.sum]).toEqual([42, 40, "b"]);
    const model = fromAnswer(merged);
    expect(model.labels.length).toBe(model.series[0].length);
  });
});

describe("asking again", () => {
  // As the server answers points=300 for the view [1,000 s, 2,000 s]: 10 s buckets, padded by 150 of them.
  const answer = { spans: [299_001, 2_990_000], padded: [-500_000, 3_510_000], extent: [0, 100_000_000] };

  test("after an answer for a view, that same view asks nothing more", () => {
    expect(asks({ view: { min: 1_000_000, max: 2_000_000 }, ...answer })).toBe(false);
  });

  test("a view near the edge of the data while the history goes on, or of another width, asks", () => {
    expect(asks({ view: { min: 2_400_000, max: 3_400_000 }, ...answer })).toBe(true);
    expect(asks({ view: { min: 1_000_000, max: 1_200_000 }, ...answer })).toBe(true);
    expect(asks({ view: { min: 0, max: 2_995_000 }, ...answer })).toBe(true);
    // At the history's own start there is nothing more to the left.
    expect(asks({ view: { min: 0, max: 1_000_000 }, ...answer, padded: [0, 3_510_000], extent: [0, 1e8] })).toBe(false);
  });

  test("the kept overview is not used after a rev change or after prices are switched on", () => {
    const kept = { rev: undefined, answers: new Map() };
    const whole = query({ points: 300, asset: null, prices: false, buys: false });
    remember(kept, whole, { rev: 41 });
    expect(kept.answers.has(whole)).toBe(true);
    expect(kept.answers.has(query({ points: 300, asset: null, prices: true, buys: false }))).toBe(false);
    remember(kept, query({ points: 300, asset: "BTC", prices: false, buys: false }), { rev: 42 });
    expect(kept.answers.has(whole)).toBe(false);
    // A narrowed view's answer is never kept.
    const narrowed = query({ points: 300, asset: null, prices: false, buys: false, view: { min: 1.5, max: 9.2 } });
    expect(narrowed).toBe("from=1&points=300&to=10");
    remember(kept, narrowed, { rev: 42 });
    expect(kept.answers.has(narrowed)).toBe(false);
  });
});

test("a gesture's view stays inside the history and no narrower than its floor", () => {
  expect(clampView(-50, 50, [0, 1000], 10)).toEqual({ min: 0, max: 100 });
  expect(clampView(950, 1050, [0, 1000], 10)).toEqual({ min: 900, max: 1000 });
  expect(clampView(500, 501, [0, 1000], 10)).toEqual({ min: 500, max: 510 });
  expect(clampView(-Infinity, Infinity, [0, 1000], 10)).toBeNull();
  expect(clampView(0, 10, [5, 5], 10)).toBeNull();
});

test("the end dot is drawn only while the dataset's last point is in view", () => {
  expect(endRadius(2, 3, 50, { min: 0, max: 50 })).toBe(3);
  expect(endRadius(2, 3, 60, { min: 0, max: 50 })).toBe(0);
  expect(endRadius(1, 3, 40, { min: 0, max: 50 })).toBe(0);
});

describe("the date axis", () => {
  const texts = (min, max) => axisMarks(min, max, 400).map(({ at, format }) => new Date(at).toLocaleString("en", format));

  test("10:10 to 10:20 has labels", () => {
    const min = +new Date(2026, 10, 19, 10, 10);
    const marks = texts(min, min + 600_000);
    expect(marks.length).toBeGreaterThanOrEqual(4);
    expect(marks[0]).toMatch(/10:10/);
  });

  test("a 40-second view has labels", () => {
    const min = +new Date(2026, 10, 19, 10, 10, 5);
    const marks = texts(min, min + 40_000);
    expect(marks.length).toBeGreaterThanOrEqual(3);
    expect(marks.every((text) => /10:1\d/.test(text))).toBe(true);
  });

  test("midnight in a minutes view is the date", () => {
    const min = +new Date(2026, 10, 19, 23, 55);
    expect(texts(min, min + 600_000)).toContain(new Date(2026, 10, 20).toLocaleString("en", { day: "numeric", month: "short" }));
  });
});
