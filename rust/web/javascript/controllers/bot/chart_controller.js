import { Controller } from "@hotwired/stimulus";
import Chart, { Interaction } from "chart.js/auto";
import { getRelativePosition } from "chart.js/helpers";
import "chartjs-adapter-date-fns";
import {
  fromAnswer, fromAttributes, keeps, query, endRadius, buysIn, curve, append, remember, inView, reading, asks, clampView,
  nearest, axisMarks, measured,
} from "../../lib/chart_series";

// The rows of both holdings tables — the index's constituents and the coins that left it.
const HOLDING_ROWS = "#assets_metrics_table tr[data-symbol], #exited_metrics_table tr[data-symbol]";

// The buy marks under the plot, in px. Read here rather than measured: the stacks are laid
// out before the images have loaded, and an unloaded <img> measures 0.
//
// This is `.widget--chart__buys__mark` in `_widget.sass` — 2rem against this app's 8px root, so
// SIXTEEN, not the 20 a 16px root would give. It sets the overlap threshold, the edge clamp and
// the reserved height, so guessing it wrong silently over-reserves the row and collapses marks
// that do not visually overlap.
const LOGO_SIZE = 16;
// Where the "Show orders" choice lives. Not keyed by bot: it is a way of reading a chart.
const ORDERS_KEY = "bot-chart:orders";
// Where the "Show prices" choice lives, on the same terms.
const PRICES_KEY = "bot-chart:prices";
// How tall the largest buy's mark stands: 5rem against this app's 8px root. The smallest stands
// at `LOGO_SIZE`, and everything between is spread across the difference — see `#sizing`.
const MARK_MAX_HEIGHT = 40;

// The dates under the plot: `axisMarks` in lib/chart_series.js, beside the units it walks.

// 548-2: the narrowest view a gesture opens on fetched data, which can bring points down to a second apart. Data that
// are all in the page (the tracker's days) stop at about three of their points instead.
const LEAST_VIEW = 10_000;
// How long a view rests before the data in hand are checked against it.
const SETTLE = 150;

// --- who decides the hovered point --------------------------------------------------
//
// Chart.js's `index` mode finds the element nearest the pointer in ANY dataset and applies its
// INDEX to all the others. That is only safe while every dataset holds the same points — and
// they do not: LTTB decimates each one on its own triangle areas, so past `maxPointsToDraw` they
// stop sampling the same days, and the nearest element is then often a price line's, whose index
// read against the curve means a different date. Nothing looks wrong when it happens: the date,
// the money, the percent and the dashed cursor all move to the same wrong day together. Measured
// on a 20-asset index at a 60-point budget, 132 of 744 pointer positions along the plot resolved
// to the wrong point, up to five days out.
//
// So the CURVE decides, and the index fans out exactly as `index` does. This also closes the
// same hole between the value and invested lines, which has been there as long as decimation has
// — `#updateSummary` works around its symptom by reading invested at the timestamp rather than
// at the index, but the timestamp it reads was itself the borrowed one.
//
// Registered on the `Interaction` registry — a NAMED export, not a static on the Chart class,
// which is `undefined` there and would throw while this module is still being imported, taking
// the whole bundle down with it.
Interaction.modes.curveIndex = (chart, event, options, useFinalPosition) => {
  const { x } = getRelativePosition(event, chart);
  const curve = chart.getDatasetMeta(0)?.data || [];
  // Only a point inside the plot: the padding loaded either side of a narrowed view is drawn under the clip, never read.
  const index = nearest(curve.map((element) => element.getProps(["x"], useFinalPosition).x), x,
                        chart.chartArea.left, chart.chartArea.right);
  if (index < 0) return [];

  return chart.getSortedVisibleDatasetMetas().flatMap((meta) => {
    const element = meta.data[index];
    return element && !element.skip ? [{ element, datasetIndex: meta.index, index }] : [];
  });
};

// Connects to data-controller="bot--chart"
export default class extends Controller {
  static targets = [
    "analyzerChart", "summary", "date", "pnl", "percent", "buys", "axis", "orders",
    "prices",
  ];
  static values = {
    series: Array,
    labels: Array,
    quote: String,
    // Set only where the money is the reader's own denomination — the tracker — and then the
    // headline carries that symbol, on the side that currency puts it, instead of the quote code.
    unit: String,
    suffixed: Boolean,
    decimals: Number,
    bot: Number,
    pnl: Array,
    assets: Object,
    pnlOnly: Boolean,
    buys: Array,
    prices: Object,
    buyLogos: Object,
    // A bot that only sells: "known" when what the sold coins cost is known, "unknown" when it is not. Absent for any
    // other bot. Its series are what it received and that cost; with the cost unknown there is no P/L at all.
    received: String,
    // D-CHART-DATA-REQUEST: the bot chart's shell names where its series are asked for, and the revision of the record
    // it was rendered from. The tracker writes the attributes above instead, and has no `src`.
    src: String,
    rev: Number,
  };

  // Stimulus parses a value's JSON again on every read. On a bot with many assets these attributes
  // run to megabytes, and one click reads them many times, so a filter that only picks a subset
  // froze the page for seconds. Each is parsed once, and again only when its attribute changes.
  #parsed = {};
  #attribute(name) {
    if (!(name in this.#parsed)) this.#parsed[name] = this[`${name}Value`];
    return this.#parsed[name];
  }

  // What the chart draws from, in one shape (lib/chart_series.js): the answer in hand, or the attributes read once.
  get #model() {
    if (this.hasSrcValue) return this.answer;
    this.attributes ||= fromAttributes(Object.fromEntries(
      ["labels", "series", "pnl", "assets", "buys", "prices", "buyLogos"].map((name) => [name, this.#attribute(name)])
    ));
    return this.attributes;
  }

  #data(name) {
    return this.#model?.[name];
  }
  seriesValueChanged() { this.#changed("series"); }
  labelsValueChanged() { this.#changed("labels"); }
  pnlValueChanged() { this.#changed("pnl"); }
  assetsValueChanged() { this.#changed("assets"); }
  buysValueChanged() { this.#changed("buys"); }
  pricesValueChanged() { this.#changed("prices"); }
  buyLogosValueChanged() { this.#changed("buyLogos"); }

  // A morph copies the attributes one at a time: one redraw for all of them, once they are all in.
  #changed(name) {
    delete this.#parsed[name];
    this.attributes = null;
    if (!this.chart || this.refreshQueued) return; // Stimulus reports every value before connect, too
    this.refreshQueued = true;
    queueMicrotask(() => {
      this.refreshQueued = false;
      if (this.chart) this.#refresh();
    });
  }

  // New figures on the chart in hand: same canvas, same mode, window, holding and toggles.
  #refresh() {
    this.stamps = null;
    if (this.days !== undefined) this.#presetView(this.days);
    this.focused = this.#chartable(this.focused) || this.#chartable(this.#pinned);
    this.pnlSeries = this.#derivePnl();
    this.#buildChart();
    // From whatever the update left hovered — Chart.js replays the last pointer event, so a pointer
    // held over the plot keeps its point, and the headline has to read that point, not the latest.
    this.#updateSummary(this.chart.tooltip);
  }

  // A revisit morphs the page into the server's answer (quiet_revisit.js). This element's insides are
  // drawn here — the canvas, the headline, the axis, the marks — so it is never morphed: it takes the
  // answer's figures and redraws. Only a different shape (the mode switch came or went, balances were
  // hidden or shown) is replaced whole.
  #morph = (event) => {
    const next = event.detail.newElement;
    if (event.target !== this.element || !next) return;

    event.preventDefault();
    if (this.#sameShape(next)) {
      for (const name of next.getAttributeNames()) {
        if (name.startsWith("data-bot--chart-") && next.getAttribute(name) !== this.element.getAttribute(name)) {
          this.element.setAttribute(name, next.getAttribute(name));
        }
      }
    } else {
      queueMicrotask(() => this.element.replaceWith(next));
    }
  };

  #sameShape(next) {
    const attribute = (element, name) => element.getAttribute(name);
    const switched = (element) => !!element.querySelector(".widget--chart__modes [data-controller~='segmented']");
    return attribute(next, "data-controller") === attribute(this.element, "data-controller") &&
      attribute(next, "data-bot--chart-pnl-only-value") === attribute(this.element, "data-bot--chart-pnl-only-value") &&
      // A morph copies attributes and never removes one: a chart whose sell-only marker changed is replaced whole.
      attribute(next, "data-bot--chart-received-value") === attribute(this.element, "data-bot--chart-received-value") &&
      switched(next) === switched(this.element);
  }

  connect() {
    // PnL is what the chart opens on, and the switch is rendered on it. A reader who picked VALUE
    // gets it back from the `segmented` control's own memory, which replays the choice as a click
    // — including after the ~5-minute metrics broadcast that destroys this controller and
    // re-renders the switch on its default.
    // A cached copy of the page (Turbo's preview) comes with the switch already on the reader's VALUE,
    // and the segmented control replays nothing it finds already on: read it off the switch.
    const on = this.element.querySelector(".widget--chart__modes [data-segmented-target='option'].is-on");
    this.currentMode = on?.dataset.value === "value" ? "value" : "pnl";
    // Seeded synchronously: the ResizeObserver below is debounced by 100ms, and a mode click
    // inside that window would otherwise rebuild with an undefined budget (Array(NaN) throws,
    // leaving the buttons in one mode and the summary in the other).
    this.maxPointsToDraw = Math.max(1, Math.floor(this.element.offsetWidth / 3.5));
    // The view in time: null for the whole history until the reader narrows it (548-2).
    this.view = null;
    // Restored HERE rather than beside the orders switch below: the summary is rendered before
    // the first build, and it reads this to decide whether it has a price to print.
    this.showPrices = this.#storedPrices;
    if (this.hasPricesTarget) this.pricesTarget.checked = this.showPrices;
    // Restored before the first build, so the marks are either there from the start or never
    // drawn at all. Like the mode switch, this outlives the page's revisits, which morph the
    // chart but leave its checkbox to the browser.
    this.showOrders = this.#storedOrders;
    if (this.hasOrdersTarget) this.ordersTarget.checked = this.showOrders;
    // Not null: a pin survives this controller, so a broadcast landing mid-read has to come back
    // on the asset the user chose rather than snapping to the portfolio while they look at it.
    this.focused = this.#chartable(this.#pinned);
    // A shell with `src` asks for its series and draws when they arrive: the plot holds the loader the chart's frame
    // shows meanwhile, and the pin is asked for with them (it is checked against the answer's holdings).
    if (this.hasSrcValue) {
      this.wanted = this.#pinned;
      this.loader = document.createElement("div");
      this.loader.className = "loader";
      this.loader.style.cssText = "position: relative; left: calc(50% - 2rem); top: calc(50% - 6rem);";
      this.analyzerChartTarget.style.display = "none";
      this.analyzerChartTarget.before(this.loader);
      this.#ask();
    }
    this.pnlSeries = this.#derivePnl();
    this.#markPinnedRow();
    this.#renderSummary();
    // Drawn HERE, not left to the observer below. That callback is debounced by 100ms and its
    // first delivery was the only thing that ever drew the chart on load — and a ResizeObserver
    // delivers nothing at all while the tab is hidden, so a chart opened in a background tab kept
    // the canvas at its authored 10x2: a curve nobody can see and a hover target a few pixels
    // wide in the plot's top-left corner. Seeding previousWidth keeps that first delivery from
    // drawing it a second time; the observer's job is REDRAWING at a new width.
    this.previousWidth = this.element.offsetWidth;
    this.#buildChart();
    this.resizeObserver = new ResizeObserver(() => {
      // Cancel any pending resize handlers
      if (this.resizeTimeout) {
        clearTimeout(this.resizeTimeout);
      }

      // Set a new timeout to handle resize after a short delay
      this.resizeTimeout = setTimeout(() => {
        const width = this.element.offsetWidth;
        if (width !== this.previousWidth) {
          this.maxPointsToDraw = Math.floor(width / 3.5);
          this.previousWidth = width;
          this.#buildChart();
          // Thinned for the old width: asked again for this one.
          if (this.answer) this.#ask();
        }
      }, 100); // 100ms delay
    });
    this.resizeObserver.observe(this.element);
    this.#gestures();
    this.element.addEventListener("turbo:before-morph-element", this.#morph);
    // D-SCHEME-SWITCH: the chart's colours are read from the page's variables when it is built, so a scheme picked in the
    // menu, or the OS turning light or dark, builds it again in the new colours.
    this.repaint = () => this.#buildChart();
    this.schemes = new MutationObserver(this.repaint);
    this.schemes.observe(document.documentElement, { attributes: true, attributeFilter: ["data-scheme", "data-mode"] });
    this.dark = matchMedia("(prefers-color-scheme: dark)");
    this.dark.addEventListener("change", this.repaint);
  }

  disconnect() {
    // An answer still on its way answers no ask of this controller's.
    this.#asked += 1;
    clearTimeout(this.focusTimer);
    clearTimeout(this.settleTimer);
    this.element.removeEventListener("turbo:before-morph-element", this.#morph);
    this.schemes.disconnect();
    this.dark.removeEventListener("change", this.repaint);
    this.resizeObserver.disconnect();
    if (this.resizeTimeout) {
      clearTimeout(this.resizeTimeout);
    }
    if (this.chart) {
      this.chart.destroy();
    }
  }

  // --- VALUE / PnL switch -----------------------------------------------------------
  //
  // The control itself (chip, aria, arrow keys) is the `segmented` controller's; this only
  // reacts to the choice. Both curves are already here, so a mode change is a redraw and
  // never a fetch.
  //
  // NOT this.mode = …: assigning that would shadow this action method.
  mode(event) {
    // The range control lives inside the modes row, so its change bubbles through here as well.
    // Named modes only — anything else is somebody else's switch.
    if (event.detail.value !== "value" && event.detail.value !== "pnl") return;

    this.currentMode = event.detail.value;
    this.#buildChart();
    this.#renderSummary();
  }

  // --- the buy marks, on or off ---------------------------------------------------
  //
  // Off by default: the marks hang over the curve, and a reader who came for the shape of the
  // line should get the line. Remembered per browser, never per bot — it is a way of reading a
  // chart, not a property of one.
  toggleOrders() {
    this.showOrders = this.ordersTarget.checked;
    try {
      localStorage.setItem(ORDERS_KEY, this.showOrders ? "1" : "0");
    } catch {
      // Not being able to remember a choice is not a reason to fail making it.
    }
    // The marks are asked for the first time they are shown.
    if (this.hasSrcValue && (!this.answer || (this.showOrders && !this.answer.buys))) return this.#ask();
    this.#renderBuys();
  }

  get #storedOrders() {
    try {
      return localStorage.getItem(ORDERS_KEY) === "1";
    } catch {
      return false; // storage can be denied outright (private mode, embedded webview)
    }
  }

  // --- the price lines, on or off ---------------------------------------------------
  //
  // Off by default and remembered per browser, exactly like the marks: another way of reading
  // one chart, not a property of the bot. A rebuild rather than a redraw — the lines are
  // datasets, and the plot has to be measured with them in it.
  togglePrices() {
    this.showPrices = this.pricesTarget.checked;
    try {
      localStorage.setItem(PRICES_KEY, this.showPrices ? "1" : "0");
    } catch {
      // Not being able to remember a choice is not a reason to fail making it.
    }
    // The lines are asked for the first time they are shown.
    if (this.hasSrcValue && (!this.answer || (this.showPrices && !this.answer.prices))) return this.#ask();
    this.#buildChart();
    this.#renderSummary();
  }

  get #storedPrices() {
    try {
      return localStorage.getItem(PRICES_KEY) === "1";
    } catch {
      return false;
    }
  }

  // --- the view ---------------------------------------------------------------------
  //
  // `view` is a time range, null for the whole history. The x scale is set to it; the series in hand reach past it
  // (the server pads a narrowed view, the tracker has every day), drawn under the plot's clip so the line enters and
  // leaves the plot as it would with every point loaded, and never read: the hover, the y range, the price rebasing
  // and the headline take only the points inside it. D-CHART-VIEW A: a narrowed view reads its own period — its P/L
  // runs from its first point to its last and the curve starts at zero there — which is what the tracker's 30D and 1Y
  // presets always did.
  //
  // A preset is a view from the first point at or after its cutoff to the last point, measured back from the LAST
  // point rather than from now: a history that stops updating must not silently empty the chart. ALL is the whole
  // history, the initial state, so a chart nobody touches renders exactly as it did before.
  range(event) {
    // Held, so new figures can be windowed the same way.
    this.days = Number(event.detail.value);
    this.#presetView(this.days);
    this.#viewChanged();
  }

  #presetView(days) {
    const stamps = this.#timestamps();
    if (!Number.isFinite(days) || !stamps.length) return (this.view = null);
    const cutoff = stamps.at(-1) - days * 86400000;
    const first = stamps.findIndex((stamp) => stamp >= cutoff);
    // Two points still make a line: a window narrower than the sampling would leave none.
    const from = Math.min(Math.max(first, 0), Math.max(stamps.length - 2, 0));
    this.view = from > 0 ? { min: stamps[from], max: stamps.at(-1) } : null;
  }

  // The whole history: the answer's (every point of it, not only the ones in hand), or the attributes' ends.
  get #extent() {
    const extent = this.#data("extent");
    return extent && Number.isFinite(extent[0]) ? extent : [0, 0];
  }

  // The time range on the x scale.
  get #shown() {
    const [min, max] = this.#extent;
    return this.view || { min, max };
  }

  get #narrowed() {
    return !!this.view;
  }

  // The indices of the points inside the view, [first, end).
  get #span() {
    return inView(this.#timestamps(), this.#shown);
  }

  // The points of a dataset the y range and the price lines are measured on (`measured`).
  #inMeasure(points) {
    const { min, max } = measured(this.#timestamps(), this.#shown);
    return points.filter((point) => point.x >= min && point.x <= max);
  }

  #viewChanged() {
    this.pnlSeries = this.#derivePnl();
    this.#buildChart();
    if (this.chart) this.#updateSummary(this.chart.tooltip);
    this.analyzerChartTarget.style.cursor = this.view ? (this.#dragging ? "grabbing" : "grab") : "";
  }

  // A gesture's view: kept inside the history (`clampView`), drawn now from the data in hand, and checked against them
  // once it has rested (`#settle`). It is no preset any more: the range control's chip goes out, and comes back on ALL
  // when the gesture reaches the whole history.
  #setView(min, max) {
    const labels = this.#timestamps();
    const [low, high] = this.#extent;
    const least = this.#data("fetched") ? LEAST_VIEW : ((high - low) / Math.max(labels.length - 1, 1)) * 3;
    this.view = clampView(min, max, [low, high], least);
    this.days = undefined;
    // An answer on its way was asked for another view: it is not drawn over this one (`#settle` asks again for it).
    if (this.inFlight) {
      this.#asked += 1;
      this.inFlight = null;
      this.dropped = true;
    }
    this.#viewChanged();
    const range = this.#rangeControl;
    if (range && this.view) range.clear();
    else range?.element.querySelector("[data-segmented-target='option'][data-value='all']:not(.is-on)")?.click();
    clearTimeout(this.settleTimer);
    // "Everything" at once: it is in hand while its answer is kept (`remember`).
    if (!this.view) this.#settle();
    else this.settleTimer = setTimeout(() => this.#settle(), SETTLE);
  }

  get #rangeControl() {
    const element = this.element.querySelector("[data-action*='bot--chart#range'] [data-controller~='segmented']");
    return element && this.application.getControllerForElementAndIdentifier(element, "segmented");
  }

  // 548-2: asked for the view when the data in hand no longer fit it (`asks`). The server pads; the view does not move
  // when the answer lands.
  #settle() {
    if (!this.hasSrcValue || !this.answer) return;
    const dropped = this.dropped;
    this.dropped = false;
    if (this.#dataKey !== this.#query() && (dropped || !this.view || asks({ view: this.#shown, ...this.answer }))) this.#ask();
  }

  // --- the gestures (as on the mock Jan approved, 2026-11-19) -------------------------
  //
  // Pinch, or ctrl/⌘ + wheel, zooms about the pointer; a mouse drag or a sideways scroll pans; a double-click returns
  // to the whole history. A plain vertical scroll is the page's. On a touch screen one finger still reads along the
  // curve, and two zoom and pan. Written by hand: no plugin.
  //
  // The headline follows a drag or a pinch frame by frame: under A the figure under a pointer that stays on its point
  // changes as the view's first point moves, and it is shown changing (Jan 2026-11-19).
  get #dragging() {
    return !!(this.drag?.moved && this.drag.view) || !!this.pinch;
  }

  #zoom(at, factor) {
    const { min, max } = this.#shown;
    const span = (max - min) * factor;
    const left = at - ((at - min) * span) / (max - min);
    this.#setView(left, left + span);
  }

  #pan(by) {
    if (this.view) this.#setView(this.view.min + by, this.view.max + by);
  }

  #gestures() {
    const canvas = this.analyzerChartTarget;
    // The page keeps the vertical scroll; sideways and pinch are this chart's.
    canvas.style.touchAction = "pan-y";
    const px = (event) => event.clientX - canvas.getBoundingClientRect().left;
    const perPixel = () => {
      const { min, max } = this.#shown;
      return (max - min) / this.chart.scales.x.width;
    };
    const timeAt = (x) => this.#shown.min + (x - this.chart.scales.x.left) * perPixel();

    canvas.addEventListener("wheel", (event) => {
      if (!this.chart) return;
      const lines = event.deltaMode === 1 ? 16 : 1;
      if (event.ctrlKey || event.metaKey) {
        event.preventDefault();
        const delta = Math.max(Math.min(event.deltaY * lines, 50), -50);
        this.#zoom(timeAt(px(event)), Math.exp(delta * 0.01));
      } else if (Math.abs(event.deltaX) > Math.abs(event.deltaY)) {
        // Also keeps the browser's back-swipe off the chart.
        event.preventDefault();
        this.#pan(event.deltaX * lines * perPixel());
      }
      // A plain vertical scroll is the page's.
    }, { passive: false });

    canvas.addEventListener("dblclick", () => {
      if (this.chart) this.#setView(-Infinity, Infinity);
    });

    this.touches = new Map();
    canvas.addEventListener("pointerdown", (event) => {
      if (!this.chart) return;
      if (event.pointerType === "mouse") {
        if (event.button === 0) this.drag = { x: event.clientX, view: this.view, perPixel: perPixel(), moved: false, id: event.pointerId };
        return;
      }
      this.touches.set(event.pointerId, px(event));
      if (this.touches.size === 2) {
        const [a, b] = [...this.touches.values()];
        this.pinch = { mid: (a + b) / 2, distance: Math.max(Math.abs(a - b), 1), shown: this.#shown, perPixel: perPixel() };
      }
    });
    canvas.addEventListener("pointermove", (event) => {
      if (event.pointerType === "mouse") {
        const drag = this.drag;
        if (!drag || !(event.buttons & 1)) return this.#endDrag();
        const dx = event.clientX - drag.x;
        if (!drag.moved && Math.abs(dx) < 4) return;
        if (!drag.moved) {
          drag.moved = true;
          try { canvas.setPointerCapture(drag.id); } catch {}
        }
        if (drag.view) this.#setView(drag.view.min - dx * drag.perPixel, drag.view.max - dx * drag.perPixel);
        return;
      }
      if (!this.touches.has(event.pointerId)) return;
      this.touches.set(event.pointerId, px(event));
      if (this.touches.size !== 2 || !this.pinch) return;
      const [a, b] = [...this.touches.values()];
      const { mid, distance, shown, perPixel: was } = this.pinch;
      // The moment under the fingers' midpoint stays under it: zoom and pan in one.
      const at = shown.min + (mid - this.chart.scales.x.left) * was;
      const scale = distance / Math.max(Math.abs(a - b), 1);
      const left = at - ((a + b) / 2 - this.chart.scales.x.left) * was * scale;
      this.#setView(left, left + (shown.max - shown.min) * scale);
    });
    const up = (event) => {
      if (event.pointerType === "mouse") return this.#endDrag();
      this.touches.delete(event.pointerId);
      if (this.touches.size < 2) this.pinch = null;
    };
    canvas.addEventListener("pointerup", up);
    canvas.addEventListener("pointercancel", up);
  }

  #endDrag() {
    const drag = this.drag;
    if (!drag) return;
    this.drag = null;
    if (drag.moved) {
      try { this.analyzerChartTarget.releasePointerCapture(drag.id); } catch {}
    }
    this.analyzerChartTarget.style.cursor = this.view ? "grab" : "";
  }

  // --- one holding at a time --------------------------------------------------------
  //
  // Pointing at a row of either holdings table draws that holding alone, on whichever mode is
  // in hand; leaving the tables restores the whole bot. Delegated from the document because the
  // tables are a sibling frame with its own broadcast — there is nothing here to bind to, and a
  // listener bound to a row would die with the next metrics cycle.
  focus(event) {
    this.#focusOn(event.target.closest?.(HOLDING_ROWS)?.dataset.symbol);
  }

  blur() {
    this.#focusOn(null);
  }

  // Clicking a row PINS it, so the pointer is free to leave the table and read along the curve —
  // which is the whole point of drawing one asset and the one thing hover alone cannot give you.
  // Clicking it again lets go. Hovering another row still previews it; the pin is what the chart
  // falls back to, not a lock.
  select(event) {
    // The quitters table puts a Sell link on its rows, and arriving at the confirmation with the
    // chart quietly re-pinned is not what that click was for.
    if (event.target.closest?.("a, button, input, label")) return;

    const symbol = event.target.closest?.(HOLDING_ROWS)?.dataset.symbol;
    if (!this.#chartable(symbol)) return;

    this.#pinned = this.#pinned === symbol ? null : symbol;
    this.#markPinnedRow();
    this.#focusOn(symbol);
  }

  // The tables are replaced by their own broadcast, which arrives on its own schedule and takes
  // the mark with it — the pin outlives the row that was wearing it. Re-applied after any stream
  // render (rAF: the swap happens in this event's default action, after the listeners).
  restore() {
    requestAnimationFrame(() => this.#markPinnedRow());
  }

  #focusOn(symbol) {
    // Falls back to the pin, not to the portfolio: moving onto the chart must not undo the choice.
    const next = this.#chartable(symbol) || this.#chartable(this.#pinned);
    if (this.hasSrcValue) return this.#want(next);
    // mouseover fires again on every cell of the row being tracked along; only a change redraws.
    if (next === this.focused) return;

    this.focused = next;
    this.#redraw();
  }

  #redraw() {
    this.pnlSeries = this.#derivePnl();
    this.#buildChart();
    this.#renderSummary();
  }

  // A holding with no marked point anywhere — every one of its points kept a fill mark — has no
  // curve to draw, so it is neither hoverable nor pinnable rather than blanking the chart.
  #chartable(symbol) {
    return symbol && this.#data("holdings")?.includes(symbol) ? symbol : null;
  }

  // --- the series, asked for (D-CHART-DATA-REQUEST) ---------------------------------
  //
  // The answer in hand holds everything drawn: the portfolio, the one holding in view, and the
  // lines and marks while they are on. Anything else is asked for, and the answer replaces the
  // one in hand. Answers of the same `rev` are kept by their query, so a holding already looked
  // at is drawn again at once; a holding not yet seen is asked for once the pointer has stayed
  // on its row for 80 ms, so running down the table asks only for the row it stops on.
  #asked = 0;
  // The whole-history answers of the current revision, by query (`remember`).
  #kept = { rev: undefined, answers: new Map() };

  #want(next) {
    if (!this.answer || next === this.wanted) return;

    this.wanted = next;
    clearTimeout(this.focusTimer);
    // Only an answer that holds every extra that is on draws the new focus from hand.
    const complete = (!this.showPrices || this.answer.prices) && (!this.showOrders || this.answer.buys);
    if (!complete) return this.#ask();
    if (!next || this.answer.asset === next) {
      // In hand: drawn now, and an ask still on its way for something else is not drawn over it.
      this.#asked += 1;
      this.focused = next;
      return this.#redraw();
    }
    if (this.#kept.answers.has(this.#query())) return this.#ask();
    // An ask on its way for another row is not drawn while this one waits its 80 ms.
    this.#asked += 1;
    this.focusTimer = setTimeout(() => this.#ask(), 80);
  }

  #query() {
    return query({ points: this.maxPointsToDraw, asset: this.wanted, prices: this.showPrices, buys: this.showOrders, view: this.view });
  }

  // The query the data in hand answered.
  get #dataKey() {
    return this.held?.key;
  }

  // `live`: a publication's new `rev` asks again for what is drawn, and when the data in hand answer that same query it
  // repeats their `closed` and `sum` (D-CHART-LIVE), so a publication that only added points sends only those.
  async #ask({ live = false } = {}) {
    clearTimeout(this.focusTimer);
    const key = this.#query();
    const ask = ++this.#asked;
    const held = live && this.held?.key === key && this.held.answer.closed != null ? this.held.answer : null;
    let answer = held ? null : this.#kept.answers.get(key);
    if (!answer) {
      const asked = held ? `${key}&after=${held.closed}&sum=${encodeURIComponent(held.sum)}` : key;
      this.inFlight = ask;
      try {
        const response = await fetch(`${this.srcValue}?${asked}`, { headers: { Accept: "application/json" }, credentials: "same-origin" });
        // 204: no figures kept yet. They are made now, and their publication's `rev` asks again.
        if (response.status !== 200) return;
        answer = await response.json();
      } catch {
        return; // the next publication's `rev` asks again
      } finally {
        if (this.inFlight === ask) this.inFlight = null;
      }
      // Kept up to the ASK's `after`, the answer's points from there on.
      if (answer.append) {
        if (!held) return;
        answer = append(held, answer, held.closed);
      }
      remember(this.#kept, key, answer);
    }
    this.#take(answer, ask, key);
  }

  // D-CHART-REV: only the latest ask is drawn, and never one older than the data in hand.
  #take(answer, ask, key) {
    if (!keeps({ ask, latest: this.#asked, rev: answer.rev, held: this.answer?.rev })) return;

    this.held = { key, answer };
    this.answer = fromAnswer(answer);
    this.stamps = null;
    this.focused = this.answer.asset === this.wanted ? this.#chartable(this.wanted) : null;
    this.wanted = this.focused;
    if (this.loader) {
      this.loader.remove();
      this.loader = null;
      this.analyzerChartTarget.style.display = "";
    }
    this.pnlSeries = this.#derivePnl();
    this.#buildChart();
    // From whatever the update left hovered: a pointer held over the plot keeps its point.
    this.#updateSummary(this.chart.tooltip);
  }

  // A publication morphed the shell (D-CHART-LIVE): a newer record asks again for what is drawn.
  revValueChanged() {
    // Stimulus reports the value before connect, too.
    if (!this.loader && !this.answer) return;
    if (this.answer && this.revValue <= this.answer.rev) return;
    // Every kept answer is of an older record now.
    this.#kept.answers.clear();
    this.#ask({ live: true });
  }

  // On the body, not on this element or in sessionStorage: it has to outlive the ~5-minute
  // broadcast that replaces the chart and the tables, and it has to NOT outlive the page — Turbo
  // swaps the body on a visit, so leaving the bot drops the pin, which is what a view-scoped
  // choice means. Keyed by bot in case a body ever survives one.
  get #pinned() {
    const [bot, symbol] = (document.body.dataset.chartPin || "").split(":");
    return bot === String(this.botValue) ? symbol : null;
  }

  set #pinned(symbol) {
    if (symbol) document.body.dataset.chartPin = `${this.botValue}:${symbol}`;
    else delete document.body.dataset.chartPin;
  }

  // The summary names the pinned asset, but the table is where the choice was made, so the row
  // has to hold it too — the pointer is somewhere over the chart by then.
  #markPinnedRow() {
    const pinned = this.#pinned;
    document.querySelectorAll(HOLDING_ROWS).forEach((row) => {
      row.classList.toggle("is-selected", row.dataset.symbol === pinned);
    });
  }

  // The value and invested curves in force: the focused holding's own, or the whole bot's.
  get #series() {
    const asset = this.focused && this.#data("assets")[this.focused];
    return asset ? [asset.value, asset.invested] : this.#data("series");
  }

  // ONE ruler across the holdings: moving from one row to the next has to move the curve, not the
  // axis. Scaled per asset, a 67 and a 2,095 holding draw the identical picture and the comparison
  // the hover exists for is lost — the reader has no way to see that one of them is the portfolio
  // and the other is a rounding error.
  //
  // Spanned over the assets ALONE, not the portfolio line: in VALUE that line is their sum, so
  // including it would cost every asset the same margin of empty frame for nothing. The portfolio
  // keeps its own scale, so the axis moves only when the pointer enters or leaves the tables.
  // Measured over every point of the history (`chart_series.js#assetBounds`), by the server for
  // fetched data: an answer carries one holding, not all of them.
  get #assetBounds() {
    return this.#data("bounds");
  }

  // Derived, not shipped: a holding's PnL is its value minus what it cost, the same subtraction
  // `chart_pnl_series` does for the portfolio. Held rather than recomputed — #pnlMode is read on
  // every pointer move.
  #derivePnl() {
    const curve = this.#rawPnl();
    // What the position had already made at the view's first point (A). Held, because the headline
    // subtracts the same figure — the curve and the number over it have to agree.
    this.baseline = reading(this.#timestamps(), curve, this.#shown, this.#narrowed).baseline;
    if (!this.baseline) return curve;

    return curve.map((point) => (point === null ? null : point - this.baseline));
  }

  #rawPnl() {
    if (!this.focused) {
      // Received with its cost unknown is no profit: there is no P/L curve to draw (the server's `pnl` was empty).
      return this.#receivedOnly ? [] : this.#data("pnl") || [];
    }

    const [value, invested] = this.#series;
    return value.map((amount, i) => (amount === null ? null : amount - invested[i]));
  }

  // A sell-only bot whose cost is not known: the curve is money in, never a profit or a loss.
  get #receivedOnly() {
    return this.receivedValue === "unknown";
  }

  // Requires an actual curve: the plot falls back to VALUE when there is none.
  get #pnlMode() {
    return this.currentMode === "pnl" && this.pnlSeries.some((point) => point !== null);
  }

  // --- summary above the chart: date, PnL in quote currency, PnL in % ------------------

  get #locale() {
    return document.documentElement.lang || "en";
  }

  // Every point in hand, the padding either side of a narrowed view included.
  #timestamps() {
    this.stamps ||= this.#data("labels") || [];
    return this.stamps;
  }

  // A day, and the time of day once the view is narrower than two days (its points are then hours or minutes apart and
  // a date alone would name them all alike); the seconds below ten minutes.
  #date(timestamp) {
    const width = this.#shown.max - this.#shown.min;
    return new Date(timestamp).toLocaleString(this.#locale, {
      year: "numeric",
      month: "short",
      day: "numeric",
      ...(width < 2 * 864e5 && { hour: "numeric", minute: "2-digit" }),
      ...(width < 6e5 && { second: "2-digit" }),
    });
  }

  // Money to the cent above a unit and to the quote's own precision below it — a BTC-quoted
  // bot's profit is a fraction of a coin, and "0.00 BTC" would say there was none.
  #number(value, { signed = true } = {}) {
    const decimals = Math.abs(value) >= 1 ? 2 : this.decimalsValue;
    return value.toLocaleString(this.#locale, {
      minimumFractionDigits: Math.min(2, decimals),
      maximumFractionDigits: decimals,
      signDisplay: signed ? "exceptZero" : "auto",
    });
  }

  // The text form, for a price on the date line or a tooltip. `signed` off for a PRICE: the
  // headline is a profit and wants its sign, but "+84,000 USDT" as the price of a fill reads as
  // a gain.
  #money(value, { signed = true } = {}) {
    return `${this.#number(value, { signed })} ${this.quoteValue}`;
  }

  // The headline: sign, then the unit in <small> — before the figure, or after it where the
  // currency is written that way. A bot's chart names its quote by its code, trailing: USDT is the
  // currency the money was actually in, and a code is how that is said; the symbol is the
  // tracker's, for the reader's own denomination.
  #headline(value, { signed = true } = {}) {
    const unit = document.createElement("small");
    unit.textContent = this.hasUnitValue ? this.unitValue : this.quoteValue;
    const suffixed = this.hasUnitValue ? this.suffixedValue : true;
    if (suffixed) return [`${this.#number(value, { signed })} `, unit];
    const sign = !signed ? "" : value > 0 ? "+" : value < 0 ? "-" : "";
    return [sign, unit, this.#number(Math.abs(value), { signed: false })];
  }

  #percent(fraction) {
    return fraction.toLocaleString(this.#locale, {
      style: "percent",
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
      signDisplay: "exceptZero",
    });
  }

  // Without a hovered point: the full date range and the bot's current PnL — which is the
  // last chart point, the same numbers the balances widget shows.
  //
  // The SAME in both modes, deliberately: PnL redraws the curve, it does not change what the
  // bot made. That is what lets the switch go uncaptioned — the reader sees one headline and
  // two ways of drawing the history behind it.
  #renderSummary(point = null) {
    if (!this.#model) return;
    const [value, invested] = this.#series;
    if (!value?.length) return;

    const timestamps = this.#timestamps();
    // The last point IN VIEW that HAS a value: a focused holding's tail is null wherever the portfolio
    // point there kept its fill mark, and reading the headline off it would print NaN. A view with no
    // point in it reads nothing: its dates, and a dash where the figures go.
    const { first: opening, last } = reading(timestamps, value, this.#shown, this.#narrowed);
    if (last < 0) return this.#renderEmpty();

    const at = point || { x: timestamps[last], value: value[last], invested: invested[last] };
    const spent = Number(at.invested);
    const pnl = Number(at.value) - spent - this.baseline;
    const first = this.#date(timestamps[opening]);
    const on = this.#date(at.x);

    // SYMBOL · price · when, each part earning its own place. The name is there whenever one
    // holding is in view — the curve would otherwise change under the pointer with nothing saying
    // to what — and the price whenever the overlay is on with a single asset to name it for.
    const range = point || first === on ? on : `${first} – ${on}`;
    const priced = this.#namedPrice;
    this.dateTarget.textContent = [this.focused || priced, this.#priceAt(priced, at.x), range]
      .filter(Boolean)
      .join(" · ");
    if (this.#receivedOnly) {
      // What came in, as an amount: no sign, no percent, no profit or loss colour. A narrowed window says what came in
      // within it, read from its first point as the P/L's baseline is.
      const start = reading(timestamps, value, this.#shown, this.#narrowed).baseline;
      if (this.hasPnlTarget) this.pnlTarget.replaceChildren(...this.#headline(Number(at.value) - start, { signed: false }));
      this.percentTarget.textContent = "";
      this.summaryTarget.classList.remove("text-danger", "text-success");
      return;
    }
    // Absent while balances are hidden: the view drops the element rather than hiding it, and
    // Stimulus raises on a missing target.
    if (this.hasPnlTarget) this.pnlTarget.replaceChildren(...this.#headline(pnl));
    // Nothing invested is not "0.00%", it is a percentage of nothing — say so rather than print a
    // return the reader could take for a flat one.
    this.percentTarget.textContent = spent > 0 ? this.#percent(pnl / spent) : "—";
    this.summaryTarget.classList.toggle("text-danger", pnl < 0);
    this.summaryTarget.classList.toggle("text-success", pnl >= 0);
  }

  #renderEmpty() {
    const { min, max } = this.#shown;
    this.dateTarget.textContent = [this.focused, `${this.#date(min)} – ${this.#date(max)}`].filter(Boolean).join(" · ");
    if (this.hasPnlTarget) this.pnlTarget.textContent = "—";
    this.percentTarget.textContent = "";
    this.summaryTarget.classList.remove("text-danger", "text-success");
  }

  // The asset whose price the headline can state: one, and only while the overlay is on. Across
  // several the overlay compares shapes and there is no single price to name; with the overlay
  // off there is no line for a number to belong to.
  get #namedPrice() {
    const symbols = this.#pricedSymbols;
    return this.showPrices && symbols.length === 1 ? symbols[0] : null;
  }

  // A point the candle grid never reached has no price; the curve skips it and so does this.
  #priceAt(symbol, timestamp) {
    const price = symbol && this.#priceSeries(symbol)[this.#indexAt(timestamp)];
    return price == null ? null : this.#money(Number(price), { signed: false });
  }

  #updateSummary(tooltip) {
    if (!tooltip.getActiveElements().length) return this.#renderSummary();

    const points = tooltip.dataPoints;
    if (this.#pnlMode) {
      // Dataset 0 by name, not `points[0]`: the price overlay adds datasets behind it, and its
      // y is a rebased ratio that would print as the bot's money.
      const parsed = points.find((point) => point.datasetIndex === 0)?.parsed;
      // One dataset in this mode, and its y is a ratio — the money behind it comes from the
      // source series at that timestamp, so the headline stays identical to VALUE mode.
      return parsed && this.#renderSummary({
        x: parsed.x,
        value: this.#valueAt(parsed.x),
        invested: this.#investedAt(parsed.x),
      });
    }

    const value = points.find((p) => p.datasetIndex === 0)?.parsed;
    if (!value) return;

    // Read invested from the source series at the hovered timestamp, NOT from the other
    // dataset's point at the same index: LTTB decimates each dataset on its own triangle
    // areas, so past `maxPointsToDraw` the two datasets no longer sample the same points and
    // index-mode would pair a value with an invested from a different day.
    this.#renderSummary({ x: value.x, value: value.y, invested: this.#investedAt(value.x) });
  }

  #valueAt(timestamp) {
    return Number(this.#series[0][this.#indexAt(timestamp)]);
  }

  // The invested total in force at a timestamp: the last transaction at or before it. Binary
  // search — the labels are sorted and this runs on every pointer move.
  #investedAt(timestamp) {
    return Number(this.#series[1][this.#indexAt(timestamp)]);
  }

  #indexAt(timestamp) {
    const timestamps = this.#timestamps();
    let low = 0;
    let high = timestamps.length - 1;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if (timestamps[middle] <= timestamp) low = middle;
      else high = middle - 1;
    }
    return low;
  }

  #buildChart() {
    if (!this.#model) return;
    const labels = this.#timestamps();
    // Held, not closed over: the plugins below belong to the chart for its whole life, and every
    // build after the first updates that chart in place.
    this.investedColor = this.#color("--benchmark");
    this.plot = this.#plot(labels);
    const datasets = this.plot.datasets;
    const maxPointsToDraw = this.plot.points;

    Chart.defaults.font.family = getComputedStyle(document.documentElement).getPropertyValue('--font-family').trim() || 'Montserrat';
    const options = this.#options(maxPointsToDraw);
    if (this.chart) {
      // The same chart on the same canvas: a rebuild would reset the canvas to its authored 10x2
      // and drop the pointer the reader is holding over it.
      this.chart.data.datasets = datasets;
      this.chart.options = options;
      this.chart.update("none");
    } else {
      this.chart = new Chart(this.#canvasContext(), { type: "line", plugins: this.#plugins(), data: { datasets }, options });
    }
    this.#renderBuys();
    this.#renderAxis();
  }

  #plugins() {
    return [
      {
        // The zero line the PnL curve is read against, under the datasets so the curve and its
        // ribbon sit on top of it. VALUE has no baseline — its own invested curve is one.
        beforeDatasetsDraw: (chart) => {
          const plot = this.plot;
          if (!plot.baseline) return;

          const y = chart.scales.y.getPixelForValue(plot.baseline.value);
          if (y < chart.chartArea.top || y > chart.chartArea.bottom) return;
          const ctx = chart.ctx;
          ctx.save();
          ctx.beginPath();
          ctx.setLineDash([3, 3]);
          ctx.moveTo(chart.chartArea.left, y);
          ctx.lineTo(chart.chartArea.right, y);
          ctx.lineWidth = 1;
          ctx.strokeStyle = plot.baseline.color;
          ctx.stroke();
          ctx.restore();
        },
      },
      {
        // Dashed cursor line through the hovered point.
        afterDatasetsDraw: (chart) => {
          if (!chart.tooltip?._active?.length) return;

          const x = chart.tooltip._active[0].element.x;
          const ctx = chart.ctx;
          ctx.save();
          ctx.beginPath();
          ctx.setLineDash([3, 3]);
          ctx.moveTo(x, chart.chartArea.top);
          ctx.lineTo(x, chart.chartArea.bottom);
          ctx.lineWidth = 1;
          ctx.strokeStyle = this.investedColor;
          ctx.stroke();
          ctx.restore();
        },
      },
    ];
  }

  #options(maxPointsToDraw) {
    return {
      responsive: true,
      maintainAspectRatio: true,
      // Off, or every cursor move restarts the point-radius transition and the hover dot
      // throbs while you track along the line.
      animation: false,
      layout: {
        padding: { top: 8, right: 4 },
      },
      scales: {
        // No axes: the summary carries the numbers and the shape carries the rest.
        // The view, or the whole history: set, not left to the datasets, which reach past a narrowed view.
        x: { type: "time", display: false, min: this.#shown.min, max: this.#shown.max },
        y: this.plot.y,
      },
      plugins: {
        legend: {
          display: false,
        },
        // The numbers live in the summary above the chart, not in a floating box.
        tooltip: {
          enabled: false,
          intersect: false,
          external: ({ tooltip }) => this.#updateSummary(tooltip),
        },
        decimation: {
          enabled: true,
          algorithm: "lttb",
          samples: maxPointsToDraw,
          threshold: maxPointsToDraw - 1,
          // Fetched series are thinned by the server already, alike across datasets.
          enabled: !this.#data("fetched"),
        },
      },
      parsing: false,
      interaction: {
        intersect: false,
        mode: "curveIndex",
      },
    };
  }

  // --- the buys under the plot -------------------------------------------------------
  //
  // One mark per executed buy, placed on the chart's own x scale so a mark sits under the point
  // it moved, and hanging off the floor of the plot itself rather than in a strip below it.
  // Marks whose logos would overlap by more than half their width collapse into one stack — and
  // a stack carries each asset ONCE, summing what that asset bought across the crowd, so an
  // index bot buying eleven coins in one second reads as eleven assets rather than as however
  // many fills the exchange split them into.
  #renderBuys() {
    if (!this.hasBuysTarget) return;
    // Nothing is drawn at all while the marks are off — on a twenty-asset index this is hundreds
    // of elements, and hiding them in CSS would build them anyway on every redraw.
    if (!this.showOrders) return this.buysTarget.replaceChildren();

    const scale = this.chart.scales.x;
    const size = LOGO_SIZE;
    // BOTH ends, not just the near one. The chart's metrics are cached for minutes while the
    // transactions are read live, so a buy that landed since the last metrics pass is newer than
    // the last label — and the clamp below would then park its logo on the final point, dating a
    // purchase to a day it did not happen on. A mark the curve cannot place is not drawn.
    // The scale's bounds (the view, or the history's own ends), not the points in hand: those reach
    // into the padding, and a buy is placed by its own time.
    const [from, to] = [scale.min, scale.max];
    const stacks = [];

    // Outside the window in view, or on an asset the reader is not looking at, there is nothing
    // to mark: the curve above does not go there either.
    buysIn(this.#data("buys"), { from, to, focused: this.focused })
      .forEach(([time, symbol, amount, quote, fills]) => {
        const x = scale.getPixelForValue(time);
        // Chained against the LAST mark, not the stack's first: a run of buys a few pixels apart
        // is one crowd, and measuring from the stack's origin would let it drift apart again.
        let stack = stacks.at(-1);
        if (!stack || x - stack.last >= size / 2) {
          stack = { from: x, to: x, last: x, assets: new Map() };
          stacks.push(stack);
        }
        stack.last = x;
        stack.to = x;

        const buy = stack.assets.get(symbol) ||
          { symbol, amount: 0, quote: 0, fills: 0, first: time, last: time };
        buy.amount += amount;
        buy.quote += quote;
        buy.fills += fills;
        buy.last = time;
        stack.assets.set(symbol, buy);
      });

    const width = scale.right;
    this.buysTarget.replaceChildren(...stacks.map((stack) => this.#stack(stack, width, this.#sizing(stacks))));
  }

  // The range the marks are drawn against, or null to leave every mark its own square.
  //
  // Marks grow only while ONE asset is in view — pinned from the holdings table, or a bot that
  // only ever bought the one thing, which is the same question and so the same test. Across
  // several assets a height would invite a comparison it cannot support: the marks of a stack are
  // already ranked by size, and marks of different assets in different stacks share no ruler.
  //
  // Measured on the AMOUNT, not the money. A DCA bot spends the same figure every time — that is
  // what makes it a DCA bot — and an index bot splits that same figure by fixed weights, so the
  // quote of a given asset's buy is constant by construction and every mark would be the same
  // height. What actually moves is how much the money bought, which is also the thing the reader
  // came for: the weeks the price was low are the tall ones.
  //
  // Comparable only because this runs with ONE asset in view; across assets the amounts are in
  // different units and only the money is a shared ruler, which is why the ORDER of marks within
  // a stack is still by quote.
  //
  // MIN-MAX against the buys IN VIEW: the smallest stands at `LOGO_SIZE` and the largest at
  // `MARK_MAX_HEIGHT`, whatever the numbers are, so the full height is always spent on the spread
  // that is actually there. Height is therefore a RANK within the window and not a quantity — a
  // buy half the size of another is not half as tall — which is the trade for two buys a hair
  // apart being legibly apart. Narrowing the range renormalizes: the question is which of THESE
  // buys was the big one, not how they measure against a month scrolled off the side.
  //
  // Buys that really are all identical have no spread to spend and are drawn at full height:
  // every one of them is the largest, and the alternative reads as the marks having no sizes.
  #sizing(stacks) {
    const buys = stacks.flatMap((stack) => [...stack.assets.values()]);
    if (new Set(buys.map((buy) => buy.symbol)).size !== 1) return null;

    const { low, high } = this.#extremes(buys.map((buy) => buy.amount));
    return { min: low, span: high - low };
  }

  // A crowd of buys as one column, and the card that reads it out.
  #stack({ from, to, assets }, width, scale) {
    const column = document.createElement("div");
    column.className = "widget--chart__buys__stack";
    // Centred on the crowd it stands for, then kept inside the plot so an edge mark is whole.
    // Offset by half a logo here rather than with a translate: a transform would make this the
    // containing block for anything positioned against the viewport inside it.
    const centre = Math.min(Math.max((from + to) / 2, LOGO_SIZE / 2), width - LOGO_SIZE / 2);
    column.style.left = `${centre - LOGO_SIZE / 2}px`;
    // The card sits on whichever side has the room. Decided from the mark's own place on the
    // axis, so the one thing that could push it off-screen is the one thing it is measured on.
    if (centre > width / 2) column.classList.add("widget--chart__buys__stack--flip");

    // SMALLEST first, because the last one painted is the one on top: the biggest buy of the
    // crowd is the one wearing its logo while the stack is closed, and the also-rans are the
    // slivers behind it. By MONEY, not by amount: a stack holds different assets, and 800 of one
    // token against 0.001 of another ranks nothing. Height, which only ever runs with a single
    // asset in view, measures the amount instead — see `#sizing`.
    const buys = [...assets.values()].sort((a, b) => a.quote - b.quote);
    // EVERY buy's reading is in the card at once, stacked in one grid cell with all but one
    // hidden. The card is therefore already as wide as the widest line it will ever hold, so
    // moving between marks swaps the text inside a box that never changes size — measuring the
    // strings would only guess at it, since a digit and a ticker are not the same width.
    const card = document.createElement("div");
    card.className = "widget--chart__buys__tip";
    card.append(...buys.map((buy) => this.#tipBuy(buy)));
    // Opened on the one the reader can actually see: the logo on top is the mark they aimed at.
    this.#showBuy(card, buys.length - 1);

    column.append(...buys.map((buy, i) => this.#mark(buy, i, scale)), card);
    // One card per stack, held open by the stack's own :hover — moving from one mark to the next
    // changes what it says instead of tearing it down and building it again a few pixels over.
    column.addEventListener("mouseover", (event) => {
      const mark = event.target.closest(".widget--chart__buys__mark");
      if (mark) this.#showBuy(card, Number(mark.dataset.buy));
    });
    return column;
  }

  // A coloured disc wearing the asset's logo. The disc is the mark: the logo on top of it fades
  // in, so a closed stack shows one logo and a column of colours, and an open one shows them all.
  // A symbol the venue has no ticker for has no image and keeps the grey the tables fall back to,
  // rather than dropping the purchase.
  // The mark IS the bar: with one asset in view it grows out of the baseline in that asset's own
  // colour, with its logo sitting at the foot, so the size of a purchase is the shape of its mark
  // rather than a second thing drawn next to it.
  #mark(buy, index, scale) {
    const mark = this.#disc(buy.symbol, "widget--chart__buys__mark");
    mark.dataset.buy = index;
    if (scale) {
      // No span is not a division to guard against, it is every buy tying for largest.
      const share = scale.span ? (buy.amount - scale.min) / scale.span : 1;
      mark.style.height = `${LOGO_SIZE + share * (MARK_MAX_HEIGHT - LOGO_SIZE)}px`;
    }
    return mark;
  }

  // A coloured disc carrying the asset's logo. The colour is always painted and the image sits
  // over it, so a symbol the venue has no ticker for still gets a disc — in the grey the holdings
  // tables fall back to — rather than the purchase silently going unmarked.
  #disc(symbol, className) {
    const asset = this.#data("logos")[symbol] || {};
    const disc = document.createElement("span");
    disc.className = className;
    disc.style.background = asset.color || "#8A9BA8";
    if (!asset.image) return disc;

    const logo = document.createElement("img");
    logo.className = "asset-logo";
    logo.src = asset.image;
    logo.alt = symbol;
    logo.loading = "lazy";
    disc.append(logo);
    return disc;
  }

  // What the crowd under one mark actually bought: which asset, when, how much, and the price it
  // went through at. Opened by the logo, because the reader arrived here by pointing at one and
  // the card has to say which one they hit. The price is DERIVED from the two totals rather than
  // read off a row, so a mark standing for several fills states the price the money as a whole
  // moved at.
  #tipBuy(buy) {
    const when = buy.first === buy.last
      ? this.#date(buy.first)
      : `${this.#date(buy.first)} – ${this.#date(buy.last)}`;
    // The count only earns its place when the mark is standing for more than one buy.
    const fills = buy.fills > 1 ? ` · ×${buy.fills}` : "";
    const block = document.createElement("div");
    block.className = "widget--chart__buys__tip__buy";
    block.append(
      this.#disc(buy.symbol, "widget--chart__buys__tip__logo"),
      this.#tipLine(`${when}${fills}`, "widget--chart__buys__tip__when"),
      this.#tipLine(`${this.#amount(buy.amount)} ${buy.symbol}`),
      this.#tipLine(this.#money(buy.quote / buy.amount, { signed: false }),
                    "widget--chart__buys__tip__price")
    );
    return block;
  }

  // Every reading is present; exactly one is visible. `visibility`, not `display`, because a
  // hidden block still has to take up its width — that is what holds the card still.
  #showBuy(card, index) {
    [...card.children].forEach((block, i) => block.classList.toggle("is-on", i === index));
  }

  #tipLine(text, className) {
    const line = document.createElement("div");
    if (className) line.className = className;
    line.textContent = text;
    return line;
  }

  // A base amount, which has no decimals shipped with the chart — an index bot's holdings each
  // have their own. Eight is the floor of what a crypto amount needs; trailing zeros are dropped.
  #amount(value) {
    return value.toLocaleString(this.#locale, { maximumFractionDigits: 8 });
  }

  // --- the dates under the plot ------------------------------------------------------
  //
  // Placed on the chart's own x scale, like the buy marks are, but in a row of their own BELOW
  // the plot: the marks hang off the plot's floor and grow upward, so the one place a label can
  // stand clear of them is under the whole thing. See `AXIS_UNITS` for what gets written.
  #renderAxis() {
    if (!this.hasAxisTarget) return;

    const scale = this.chart.scales.x;
    const marks = axisMarks(scale.min, scale.max, scale.width).map(({ at, format }) => {
      const mark = document.createElement("span");
      mark.textContent = new Date(at).toLocaleString(this.#locale, format);
      mark.style.left = `${scale.getPixelForValue(at)}px`;
      return mark;
    });
    this.axisTarget.replaceChildren(...marks);

    // Clamped only once they are in the DOM, because it takes their measured widths: a label
    // centred on a boundary at either end would otherwise hang half outside the widget. The
    // same trade the buy marks make — a few px off its date beats being cut in half.
    marks.forEach((mark) => {
      const half = mark.offsetWidth / 2;
      const x = parseFloat(mark.style.left);
      mark.style.left = `${Math.min(Math.max(x, half), scale.width - half)}px`;
    });
  }

  // What to draw for the mode in hand: the datasets, the y scale they need, how many points
  // survive decimation, and (RETURN only) the colour of the 100 line. Falls back to VALUE when
  // there is no return curve to show — a bot that never held anything has no return.
  // The extremes of a series WITHOUT spreading it into Math.min/Math.max. That form passes one
  // argument per point, and engines cap the argument count around 125k — a bot with years of
  // five-minute fills has a series that long on its own, and a price line per asset multiplies
  // it. The throw would not cost the reading that needed the bound, it would cost the whole
  // chart: every one of these runs inside `#buildChart`.
  //
  // The seeds are the identities each caller wants — a floor of 0 for a plot that has to keep
  // zero in frame, ±Infinity for one measuring only what is there.
  #extremes(values, low = Infinity, high = -Infinity) {
    values.forEach((value) => {
      if (value < low) low = value;
      if (value > high) high = value;
    });
    return { low, high };
  }

  #plot(labels) {
    const curve = this.#pnlMode ? this.#pnlPoints(labels) : [];
    const plot = curve.length ? this.#pnlPlot(curve) : this.#valuePlot(labels);
    return this.#withPrices(plot, labels);
  }

  // --- the price overlay -------------------------------------------------------------
  //
  // One line per asset, in its ticker's own colour, laid over whichever plot is in hand.
  //
  // REBASED, never drawn in money: a coin at 60,000 and one at 0.5 share no axis, and the money
  // axis here belongs to the curve underneath. Each line is divided by its own first price IN
  // VIEW, so every line — and the curve they are read against — leaves the same point on the
  // left edge and what the reader compares is the shape. Narrowing the range rebases again, for
  // the same reason the RETURN curve does: the question is what these weeks did.
  //
  // Every line leaves the curve's first point, whatever that costs the frame: a band that does
  // not fit around that point widens the plot by exactly what it needs, rather than sliding off
  // it — an overlay that starts somewhere else is not the comparison the reader turned on. One
  // scale across all the lines at once, so they keep their sizes relative to each other.
  //
  // The trade is that a price line carries no readable number, which is what the price under the
  // headline is for — and why it only appears when there is a single line to name.
  #withPrices(plot, labels) {
    const symbols = this.#pricedSymbols;
    if (!this.showPrices || !symbols.length) return plot;

    // Two points make a line; one is a dot nobody can read a shape off.
    const lines = symbols
      .map((symbol) => [symbol, this.#rebased(symbol, labels)])
      .filter(([, points]) => points.length > 1);
    // Where the curve starts in view, which is where every price line has to start with it.
    const anchor = plot.datasets[0].data.find((point) => point.x >= this.#shown.min)?.y;
    if (!lines.length || anchor === undefined) return plot;

    // Seeded at zero because every line's own first point in view IS zero: the rebase makes it so.
    const { low, high } = this.#extremes(
      lines.flatMap(([, points]) => this.#inMeasure(points).map((point) => point.y)), 0, 0
    );
    const scale = this.#priceScale(plot.y, anchor, low, high);

    return {
      ...plot,
      y: this.#widened(plot.y, anchor + low * scale, anchor + high * scale),
      datasets: plot.datasets.concat(
        lines.map(([symbol, points]) => this.#priceDataset(symbol, points, anchor, scale))
      ),
    };
  }

  // The plot's y range, pushed out only where the band leaves it, with a sliver of room so the
  // extreme line is not drawn along the edge. `max` rather than `suggestedMax` on the top: the
  // band is a hard extent, not a hint.
  #widened(y, bottom, top) {
    const floor = y.min ?? 0;
    const ceiling = y.max ?? y.suggestedMax;
    const pad = (ceiling - floor) * 0.02;
    const widened = { ...y };
    if (bottom - pad < floor) widened.min = bottom - pad;
    if (top + pad > ceiling) widened.max = top + pad;
    return widened;
  }

  // The assets whose prices are worth drawing: the focused holding alone, or all of them. A
  // focused symbol the chart could not price draws nothing rather than falling back to the rest.
  get #pricedSymbols() {
    const symbols = this.#data("priced") || [];
    if (!this.focused) return symbols;

    return symbols.includes(this.focused) ? [this.focused] : [];
  }

  #priceSeries(symbol) {
    return this.#data("prices")?.[symbol] || [];
  }

  // A price series as its distance from its own first price in view, nulls dropped — every point
  // carries its own timestamp, so a gap in the grid costs nothing.
  #rebased(symbol, labels) {
    const serie = this.#priceSeries(symbol);
    const [first, end] = this.#span;
    const base = serie.slice(first, end).find((price) => price != null);
    if (!base) return [];

    return serie
      .map((price, i) => ({ x: labels[i], y: price == null ? null : Number(price) / base - 1 }))
      .filter((point) => point.y !== null);
  }

  // How far a deviation of 1 travels in the plot's own units.
  //
  // The obvious answer is the largest scale that keeps every line inside the frame — whichever
  // direction runs out first, minus a tenth so the extreme line is not drawn along the edge. But
  // that room is measured from the ANCHOR, and in VALUE mode the anchor is the portfolio at its
  // first buy, sitting a hair above a hard zero floor: a DCA bot's first buy is roughly 1/N of
  // what it ends up holding, so that distance alone sets the scale. Measured on a real 20-asset
  // index, the overlay got 10.7% of the plot height and lay along the floor, unreadable. RETURN
  // mode never has the problem — its anchor sits mid-frame and the same rule spends ~70%.
  //
  // So the fit has a floor of its own: half the frame, whatever the anchor allows. Past that
  // floor the band no longer fits around the anchor, and it is the frame that gives (`#widened`)
  // — the start stays shared, and the curve moves by the room the troughs needed.
  //
  // ponytail: half is a taste threshold, not a derived one.
  #priceScale(y, anchor, low, high) {
    const spread = high - low;
    // Prices that never moved need no room at all and take any scale.
    if (!spread) return 1;

    const floor = y.min ?? 0;
    const ceiling = y.max ?? y.suggestedMax;
    const up = high > 0 ? (ceiling - anchor) / high : Infinity;
    const down = low < 0 ? (anchor - floor) / -low : Infinity;
    // One of the two is finite: a non-zero spread has a side that leaves the anchor.
    const room = Math.max(Math.min(up, down), 0);
    return Math.max(room * 0.9, ((ceiling - floor) / spread) * 0.5);
  }

  // Thinner and softer than the curve, and with no point on its end: the overlay is context, and
  // the line the reader came for has to stay the loudest thing in the frame. Grey for a symbol
  // the venue has no ticker for, the same fallback the marks and the tables use.
  #priceDataset(symbol, points, base, scale) {
    const color = this.#setTransparency(
      this.#safeColor(this.#data("logos")[symbol]?.color || "#8A9BA8"), 0.65
    );
    return {
      label: symbol,
      cubicInterpolationMode: "monotone",
      borderWidth: 1.5,
      borderColor: color,
      pointRadius: 0,
      pointHitRadius: 0,
      pointHoverRadius: 0,
      fill: false,
      data: points.map((point) => ({ x: point.x, y: base + point.y * scale })),
    };
  }

  // The same reading as the PnL plot, in money: the invested line is the zero line, the value
  // curve is green where it sits above it and red where it sits below, and the ribbon between
  // them is the PnL. A narrowed window draws the invested line from where the VALUE started —
  // lifted by the PnL the position already had, exactly what the RETURN curve subtracts — so
  // both lines leave the left edge together and their gap says what THESE weeks did, which is
  // what the headline over them says. The hover keeps reading the source series, so the money
  // printed is still what was actually invested.
  #valuePlot(labels) {
    // The baseline is 0 for the whole history (A: only a narrowed view lifts the invested line).
    const shift = this.baseline;
    // Nulls dropped, not zeroed: a focused holding has no market value at a point the portfolio
    // left on its fill mark. Every point carries its own timestamp, so a gap costs nothing.
    const series = this.#series.map((serie, i) => curve(labels, serie, i ? shift : 0));
    // Received only: the cost series is all zeros and never drawn, so it never frames the window.
    const span = this.#extremes((this.#receivedOnly ? series.slice(0, 1) : series).flatMap((serie) => this.#inMeasure(serie).map((p) => p.y)));
    const maxValue = this.focused ? this.#assetBounds.value : span.high;
    const points = Math.min(this.maxPointsToDraw, series[0].length, series[1].length);
    const up = this.#color("--grass");
    const down = this.#color("--berry");
    // The invested line as DRAWN at a timestamp.
    const invested = (x) => this.#investedAt(x) + shift;
    // The side a segment lies on, by the PnL at its two ends — NOT by the value against the
    // invested level at one end: a buy raises both between one point and the next, and read
    // against the old level that day would count as profit.
    const above = (p0, p1) => (p0.y - invested(p0.x) + p1.y - invested(p1.x)) / 2 >= 0;
    // The whole history is read from zero — that is where the money came from. A narrowed
    // window is read from its own floor: both lines sit far above zero by then, and a frame
    // kept at zero flattens what these weeks did into a line along the middle of it.
    const pad = (span.high - span.low) * 0.1 || 0.01;
    // A focused holding too, as on the mock: the ruler across holdings is the whole history's.
    const y = this.#narrowed && Number.isFinite(span.low)
      ? { min: span.low - pad, max: span.high + pad }
      : { min: 0, suggestedMax: maxValue * 1.1 };
    // One curve, money in, in the page's own ink: no cost to read it against, so no ribbon and no side.
    if (this.#receivedOnly) {
      return { points, y: { display: false, ...y }, datasets: [{ ...this.#lineDataset(this.#color("--ink"), series[0]), fill: false }] };
    }

    return {
      points,
      y: { display: false, ...y },
      datasets: [
        {
          // A holding with no marked point here has an empty curve: drawn as nothing, not as a crash.
          ...this.#lineDataset((series[0].at(-1)?.y ?? 0) >= (series[1].at(-1)?.y ?? 0) ? up : down, series[0]),
          pointHoverBorderColor: (ctx) => (ctx.parsed && ctx.parsed.y >= invested(ctx.parsed.x) ? up : down),
          segment: { borderColor: (ctx) => (above(ctx.p0.parsed, ctx.p1.parsed) ? up : down) },
          fill: {
            target: 1,
            above: this.#setTransparency(up, 0.12),
            below: this.#setTransparency(down, 0.12),
          },
        },
        // Interpolated exactly as the value curve is, and not as the step function the cash
        // actually is: the ribbon is the area between the two lines, and a step under a curve
        // paints a wedge of profit at every buy — value climbs across the day, invested lands
        // at its end — where the PnL did not move at all.
        this.#lineDataset(this.#color("--benchmark"), series[1]),
      ],
    };
  }

  // The invested line becomes the zero line, so the curve IS the distance from it, and the
  // fill is the reading: the ribbon between the curve and zero, green above and red below.
  #pnlPlot(curve) {
    // Zero stays in frame whether or not the curve reaches it: the line is what the curve means,
    // which is what the 0 seeds say.
    const curveSpan = this.#extremes(this.#inMeasure(curve).map((point) => point.y), 0, 0);
    const ruler = this.focused && !this.#narrowed;
    const low = ruler ? this.#assetBounds.low : curveSpan.low;
    const high = ruler ? this.#assetBounds.high : curveSpan.high;
    const pad = (high - low) * 0.1 || 0.01;
    const up = this.#color("--grass");
    const down = this.#color("--berry");
    const points = Math.min(this.maxPointsToDraw, curve.length);

    return {
      points,
      baseline: { value: 0, color: this.#color("--benchmark") },
      y: { display: false, min: low - pad, max: high + pad },
      datasets: [{
        // The dot and the hover ring take the end state; the LINE changes colour where it
        // crosses zero, or a curve that spent months in profit would be drawn red because it
        // happens to end a hair under water — with a green ribbon underneath it.
        ...this.#lineDataset(curve.at(-1).y >= 0 ? up : down, curve),
        // The ring follows the HOVERED point, not the end state, or hovering a loss on a chart
        // that ends in profit would draw a green ring around a red section.
        pointHoverBorderColor: (ctx) => ((ctx.parsed?.y ?? 0) >= 0 ? up : down),
        // Coloured by the side the segment mostly lies on. Chart.js resolves this once per
        // segment, so one that straddles zero takes a single colour either way; the midpoint
        // picks the larger half instead of whichever end happens to be second.
        segment: { borderColor: (ctx) => ((ctx.p0.parsed.y + ctx.p1.parsed.y) / 2 >= 0 ? up : down) },
        fill: {
          value: 0,
          above: this.#setTransparency(up, 0.12),
          below: this.#setTransparency(down, 0.12),
        },
      }],
    };
  }

  // Points before the bot had anything invested arrive as nulls, so the curve stays aligned
  // with the labels. They are dropped here — every point carries its own timestamp, and
  // decimation cannot sample around a hole.
  #pnlPoints(labels) {
    return curve(labels, this.pnlSeries);
  }

  // Shared shape of every line here: the curve, and a permanent dot on its last point that
  // swaps fill on hover instead of resizing. Scriptable, so the dot lands on the dataset's own
  // last point whatever its length: decimation's (which always keeps the last), or a fetched
  // series' after its nulls were dropped.
  #lineDataset(color, data) {
    return {
      cubicInterpolationMode: "monotone",
      borderWidth: 2,
      borderColor: color,
      // Only while that point is in view: a narrowed view ending before it has no "now" to mark.
      pointRadius: (ctx) => endRadius(ctx.dataIndex, ctx.dataset.data.length, ctx.raw?.x, this.#shown),
      pointHitRadius: 0,
      pointBackgroundColor: color,
      pointBorderColor: color,
      pointBorderWidth: 0,
      pointHoverRadius: 3,
      pointHoverBackgroundColor: this.#color("--washed"),
      pointHoverBorderColor: color,
      pointHoverBorderWidth: 2,
      data,
      // Clipped at the plot's left and right, where a narrowed view's padding runs on; the 4 px right of the plot are the
      // end dot's, as the layout's padding has them. Top and bottom unclipped, as before.
      clip: { left: 0, top: false, right: 4, bottom: false },
    };
  }

  #color(variable) {
    return this.#safeColor(this.#getCssVariableValue(variable));
  }

  #canvasContext() {
    return this.analyzerChartTarget.getContext("2d", {
      colorSpace: "display-p3",
    });
  }

  #getCssVariableValue(variableName) {
    const root = document.documentElement;
    const style = getComputedStyle(root);
    const value = style.getPropertyValue(variableName);
    return value.trim();
  }

  #displaySupportsP3Color() {
    return matchMedia("(color-gamut: p3)").matches;
  }

  #canvasSupportsDisplayP3() {
    let canvas = document.createElement("canvas");
    try {
      // Safari throws a TypeError if the colorSpace option is supported, but
      // the system requirements (minimum macOS or iOS version) for Display P3
      // support are not met.
      let context = canvas.getContext("2d", { colorSpace: "display-p3" });
      return context.getContextAttributes().colorSpace == "display-p3";
    } catch {
      return false;
    }
  }

  #canvasSupportsWideGamutCSSColors() {
    let context = document.createElement("canvas").getContext("2d");
    let initialFillStyle = context.fillStyle;
    context.fillStyle = "color(display-p3 0 1 0)";
    return context.fillStyle != initialFillStyle;
  }

  #wideGamutColorSupported() {
    return (
      this.#displaySupportsP3Color() &&
      this.#canvasSupportsDisplayP3() &&
      this.#canvasSupportsWideGamutCSSColors()
    );
  }

  #isValidDisplayP3Color(colorString) {
    const regex = this.#displayP3Regex();
    return regex.test(colorString);
  }

  #isValidHexColor(hexString) {
    const regex = this.#hexRegex();
    return regex.test(hexString);
  }

  #isValidRgbColor(rgbString) {
    const regex = this.#rgbRegex();
    const match = regex.exec(rgbString);
    if (!match) {
      return false;
    }
    const r = parseInt(match[1], 10);
    const g = parseInt(match[2], 10);
    const b = parseInt(match[3], 10);
    return r >= 0 && r <= 255 && g >= 0 && g <= 255 && b >= 0 && b <= 255;
  }

  #safeColor(color) {
    // returns a display-p3 color string if provided and supported, otherwise returns the rgba color
    if (
      this.#isValidDisplayP3Color(color) &&
      !this.#wideGamutColorSupported()
    ) {
      return this.#displayP3ToRgba(color);
    } else if (this.#isValidHexColor(color)) {
      return this.#hexToRgba(color);
    } else if (this.#isValidRgbColor(color)) {
      return this.#rgbToRgba(color);
    } else {
      return color;
    }
  }

  #displayP3ToRgba(displayP3String) {
    const match = displayP3String.match(this.#displayP3Regex());
    if (!match) {
      throw new Error("Invalid color(display-p3 ...) string");
    }
    const r = parseFloat(match[1]);
    const g = parseFloat(match[5]);
    const b = parseFloat(match[9]);
    const a = match[14] ? parseFloat(match[10].trim()) : 1;
    const srgbR = Math.round(Math.max(0, Math.min(1, r)) * 255);
    const srgbG = Math.round(Math.max(0, Math.min(1, g)) * 255);
    const srgbB = Math.round(Math.max(0, Math.min(1, b)) * 255);
    return `rgba(${srgbR}, ${srgbG}, ${srgbB}, ${a})`;
  }

  #hexToRgba(hex) {
    if (!this.#isValidHexColor(hex)) {
      throw new Error("Invalid hex color code");
    }
    hex = hex.slice(1);
    let r,
      g,
      b,
      a = 255;
    if (hex.length === 3) {
      r = parseInt(hex[0] + hex[0], 16);
      g = parseInt(hex[1] + hex[1], 16);
      b = parseInt(hex[2] + hex[2], 16);
    } else if (hex.length === 6) {
      r = parseInt(hex.slice(0, 2), 16);
      g = parseInt(hex.slice(2, 4), 16);
      b = parseInt(hex.slice(4, 6), 16);
    } else if (hex.length === 8) {
      r = parseInt(hex.slice(0, 2), 16);
      g = parseInt(hex.slice(2, 4), 16);
      b = parseInt(hex.slice(4, 6), 16);
      a = parseInt(hex.slice(6, 8), 16);
    }
    const alpha = (a / 255).toFixed(2);
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }

  #rgbToRgba(rgbString) {
    if (!this.#isValidRgbColor(color)) {
      throw new Error("Invalid rgb color code");
    }
    const match = rgbString.match(this.#rgbRegex());
    if (match) {
      const r = match[1];
      const g = match[2];
      const b = match[3];
      const a = 1;
      return `rgba(${r}, ${g}, ${b}, ${a})`;
    }
    throw new Error("Unexpected error parsing the RGB string");
  }

  #setTransparency(color, transparency) {
    // Validate transparency value
    if (transparency < 0 || transparency > 1) {
      throw new Error("Transparency value must be between 0 and 1");
    }

    let match = color.match(this.#displayP3Regex());
    if (match) {
      const r = match[1];
      const g = match[5];
      const b = match[9];
      return `color(display-p3 ${r} ${g} ${b} / ${transparency})`;
    }

    match = color.match(this.#rgbaRegex());
    if (match) {
      const r = match[1];
      const g = match[2];
      const b = match[3];
      return `rgba(${r}, ${g}, ${b}, ${transparency})`;
    }

    throw new Error("Invalid color format. Must be display-p3 or rgba.");
  }

  #displayP3Regex() {
    return /^color\(display-p3\s(\d(\.\d+)?|1(\.0+)?|0(\.0+)?|0?\.\d+)\s(\d(\.\d+)?|1(\.0+)?|0(\.0+)?|0?\.\d+)\s(\d(\.\d+)?|1(\.0+)?|0(\.0+)?|0?\.\d+)(\s\/\s(0?\.\d+|1(\.0+)?|0(\.0+)?))?\)$/;
  }

  #rgbRegex() {
    return /^rgb\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*\)$/;
  }

  #rgbaRegex() {
    return /^rgba\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(0?\.\d+|1(\.0+)?|0(\.0+)?)\s*\)$/;
  }

  #hexRegex() {
    return /^#([A-Fa-f0-9]{3}|[A-Fa-f0-9]{6}|[A-Fa-f0-9]{8})$/;
  }
}
