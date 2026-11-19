// bun test rust/web/javascript/lib — the bot chart's reading of its data (chart_series.js).
import { describe, expect, test } from "bun:test";
import { buysIn, curve, endRadius, fromAnswer, fromAttributes, keeps, query } from "./chart_series.js";

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
