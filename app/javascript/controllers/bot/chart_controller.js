import { Controller } from "@hotwired/stimulus";
import Chart from "chart.js/auto";
import "chartjs-adapter-date-fns";

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
// How tall the largest buy's mark stands: 5rem against this app's 8px root. The smallest stands
// at `LOGO_SIZE`, and everything between is spread across the difference — see `#sizing`.
const MARK_MAX_HEIGHT = 40;

// Connects to data-controller="bot--chart"
export default class extends Controller {
  static targets = ["analyzerChart", "summary", "date", "pnl", "percent", "buys", "orders"];
  static values = {
    series: Array,
    labels: Array,
    quote: String,
    decimals: Number,
    bot: Number,
    pnl: Array,
    assets: Object,
    pnlOnly: Boolean,
    buys: Array,
    buyLogos: Object,
  };

  connect() {
    // PnL is what the chart opens on, and the switch is rendered on it. A reader who picked VALUE
    // gets it back from the `segmented` control's own memory, which replays the choice as a click
    // — including after the ~5-minute metrics broadcast that destroys this controller and
    // re-renders the switch on its default.
    this.currentMode = "pnl";
    // Seeded synchronously: the ResizeObserver below is debounced by 100ms, and a mode click
    // inside that window would otherwise rebuild with an undefined budget (Array(NaN) throws,
    // leaving the buttons in one mode and the summary in the other).
    this.maxPointsToDraw = Math.max(1, Math.floor(this.element.offsetWidth / 3.5));
    // The whole history until the reader narrows it; every read of the series slices from here.
    this.from = 0;
    // Not null: a pin survives this controller, so a broadcast landing mid-read has to come back
    // on the asset the user chose rather than snapping to the portfolio while they look at it.
    this.focused = this.#chartable(this.#pinned);
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
    // Restored before the first build, so the marks are either there from the start or never
    // drawn at all. Like the mode switch, this outlives the ~5-minute broadcast that destroys
    // this controller and re-renders the checkbox on its default.
    this.showOrders = this.#storedOrders;
    if (this.hasOrdersTarget) this.ordersTarget.checked = this.showOrders;
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
        }
      }, 100); // 100ms delay
    });
    this.resizeObserver.observe(this.element);
  }

  disconnect() {
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
    this.#renderBuys();
  }

  get #storedOrders() {
    try {
      return localStorage.getItem(ORDERS_KEY) === "1";
    } catch {
      return false; // storage can be denied outright (private mode, embedded webview)
    }
  }

  // --- the window in view -----------------------------------------------------------
  //
  // CONTRACT (this repo has no JS test harness; the bot chart's own behaviour has the same
  // coverage, so this is checked by hand in the browser): `from` is the index of the first point
  // at or after the cutoff, measured back from the LAST point rather than from now — a history
  // that stops updating must not silently empty the chart. Everything downstream slices from it,
  // and both the RETURN curve and the headline are read RELATIVE to that first point, so "30D"
  // says what the last thirty days did instead of where the whole history happens to stand today.
  // ALL is the initial state, so a chart nobody touches renders exactly as it did before.
  range(event) {
    const stamps = this.#allTimestamps();
    const days = Number(event.detail.value);
    if (!Number.isFinite(days) || !stamps.length) {
      this.from = 0;
    } else {
      const cutoff = stamps.at(-1) - days * 86400000;
      const first = stamps.findIndex((stamp) => stamp >= cutoff);
      // Two points still make a line: a window narrower than the sampling would leave none.
      this.from = Math.min(Math.max(first, 0), Math.max(stamps.length - 2, 0));
    }
    this.pnlSeries = this.#derivePnl();
    this.#buildChart();
    this.#renderSummary();
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
    // mouseover fires again on every cell of the row being tracked along; only a change redraws.
    if (next === this.focused) return;

    this.focused = next;
    this.pnlSeries = this.#derivePnl();
    this.#buildChart();
    this.#renderSummary();
  }

  // A holding with no marked point anywhere — every one of its points kept a fill mark — has no
  // curve to draw, so it is neither hoverable nor pinnable rather than blanking the chart.
  #chartable(symbol) {
    const asset = symbol ? this.assetsValue?.[symbol] : null;
    return asset?.value?.some((amount) => amount !== null) ? symbol : null;
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
    const asset = this.focused && this.assetsValue[this.focused];
    const series = asset ? [asset.value, asset.invested] : this.seriesValue;
    return this.from ? series.map((serie) => serie.slice(this.from)) : series;
  }

  // ONE ruler across the holdings: moving from one row to the next has to move the curve, not the
  // axis. Scaled per asset, a 67 and a 2,095 holding draw the identical picture and the comparison
  // the hover exists for is lost — the reader has no way to see that one of them is the portfolio
  // and the other is a rounding error.
  //
  // Spanned over the assets ALONE, not the portfolio line: in VALUE that line is their sum, so
  // including it would cost every asset the same margin of empty frame for nothing. The portfolio
  // keeps its own scale, so the axis moves only when the pointer enters or leaves the tables.
  get #assetBounds() {
    this.bounds ||= Object.values(this.assetsValue).reduce(
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
    return this.bounds;
  }

  // Derived, not shipped: a holding's PnL is its value minus what it cost, the same subtraction
  // `chart_pnl_series` does for the portfolio. Held rather than recomputed — #pnlMode is read on
  // every pointer move.
  #derivePnl() {
    const curve = this.#rawPnl();
    // What the position had already made when the window opened. Held, because the headline
    // subtracts the same figure — the curve and the number over it have to agree.
    this.baseline = this.from ? curve.find((point) => point !== null) ?? 0 : 0;
    if (!this.baseline) return curve;

    return curve.map((point) => (point === null ? null : point - this.baseline));
  }

  #rawPnl() {
    if (!this.focused) {
      const portfolio = this.pnlValue || [];
      return this.from ? portfolio.slice(this.from) : portfolio;
    }

    const [value, invested] = this.#series;
    return value.map((amount, i) => (amount === null ? null : amount - invested[i]));
  }

  // Requires an actual curve: the plot falls back to VALUE when there is none.
  get #pnlMode() {
    return this.currentMode === "pnl" && this.pnlSeries.some((point) => point !== null);
  }

  // --- summary above the chart: date, PnL in quote currency, PnL in % ------------------

  get #locale() {
    return document.documentElement.lang || "en";
  }

  #allTimestamps() {
    this.stamps ||= this.labelsValue.map((date) => new Date(date).getTime());
    return this.stamps;
  }

  #timestamps() {
    return this.from ? this.#allTimestamps().slice(this.from) : this.#allTimestamps();
  }

  #date(timestamp) {
    return new Date(timestamp).toLocaleDateString(this.#locale, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }

  // `signed` off for a PRICE: the headline is a profit and wants its sign, but "+84,000 USDT"
  // as the price of a fill reads as a gain.
  #money(value, { signed = true } = {}) {
    const decimals = Math.abs(value) >= 1 ? 2 : this.decimalsValue;
    return `${value.toLocaleString(this.#locale, {
      minimumFractionDigits: Math.min(2, decimals),
      maximumFractionDigits: decimals,
      signDisplay: signed ? "exceptZero" : "auto",
    })} ${this.quoteValue}`;
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
    const [value, invested] = this.#series;
    if (!value?.length) return;

    const timestamps = this.#timestamps();
    // The last point that HAS a value: a focused holding's tail is null wherever the portfolio
    // point there kept its fill mark, and reading the headline off it would print NaN.
    let last = value.length - 1;
    while (last >= 0 && value[last] === null) last -= 1;
    if (last < 0) return;

    const at = point || { x: timestamps[last], value: value[last], invested: invested[last] };
    const spent = Number(at.invested);
    const pnl = Number(at.value) - spent - this.baseline;
    const first = this.#date(timestamps[0]);
    const on = this.#date(at.x);

    // Named while focused, or the curve would change under the pointer with nothing saying to what.
    const range = point || first === on ? on : `${first} – ${on}`;
    this.dateTarget.textContent = this.focused ? `${this.focused} · ${range}` : range;
    // Absent while balances are hidden: the view drops the element rather than hiding it, and
    // Stimulus raises on a missing target.
    if (this.hasPnlTarget) this.pnlTarget.textContent = this.#money(pnl);
    // Nothing invested is not "0.00%", it is a percentage of nothing — say so rather than print a
    // return the reader could take for a flat one.
    this.percentTarget.textContent = spent > 0 ? this.#percent(pnl / spent) : "—";
    this.summaryTarget.classList.toggle("text-danger", pnl < 0);
    this.summaryTarget.classList.toggle("text-success", pnl >= 0);
  }

  #updateSummary(tooltip) {
    if (!tooltip.getActiveElements().length) return this.#renderSummary();

    const points = tooltip.dataPoints;
    if (this.#pnlMode) {
      const parsed = points[0]?.parsed;
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
    const labels = this.#timestamps();
    const invested_color = this.#color("--benchmark");
    const plot = this.#plot(labels);
    const datasets = plot.datasets;
    const maxPointsToDraw = plot.points;

    Chart.defaults.font.family = getComputedStyle(document.documentElement).getPropertyValue('--font-family').trim() || 'Montserrat';
    if (this.chart) {
      this.chart.destroy();
    }
    this.chart = new Chart(this.#canvasContext(), {
      type: "line",
      plugins: [
        {
          // The zero line the PnL curve is read against, under the datasets so the curve and its
          // ribbon sit on top of it. VALUE has no baseline — its own invested curve is one.
          beforeDatasetsDraw: (chart) => {
            if (!plot.baseline) return;

            const y = chart.scales.y.getPixelForValue(plot.baseline.value);
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
            ctx.strokeStyle = invested_color;
            ctx.stroke();
            ctx.restore();
          },
        },
      ],
      data: {
        datasets: datasets,
      },
      options: {
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
          x: { type: "time", display: false },
          y: plot.y,
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
          },
        },
        parsing: false,
        interaction: {
          intersect: false,
          mode: "index",
        },
      },
    });
    this.#renderBuys();
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
    const timestamps = this.#timestamps();
    const from = timestamps[0];
    // BOTH ends, not just the near one. The chart's metrics are cached for minutes while the
    // transactions are read live, so a buy that landed since the last metrics pass is newer than
    // the last label — and the clamp below would then park its logo on the final point, dating a
    // purchase to a day it did not happen on. A mark the curve cannot place is not drawn.
    const to = timestamps.at(-1);
    const stacks = [];

    this.buysValue
      // Outside the window in view, or on an asset the reader is not looking at, there is nothing
      // to mark: the curve above does not go there either.
      .filter(([at, symbol]) => {
        if (this.focused && symbol !== this.focused) return false;

        const time = new Date(at).getTime();
        return time >= from && time <= to;
      })
      .forEach(([at, symbol, amount, quote, fills]) => {
        const time = new Date(at).getTime();
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
  // MIN-MAX against the buys IN VIEW: the smallest stands at `LOGO_SIZE` and the largest at
  // `MARK_MAX_HEIGHT`, whatever the numbers are, so the full height is always spent on the spread
  // that is actually there. Height is therefore a RANK within the window and not a quantity — a
  // buy half the size of another is not half as tall — which is the trade for two buys a hair
  // apart being legibly apart. Narrowing the range renormalizes: the question is which of THESE
  // buys was the big one, not how they measure against a month scrolled off the side.
  //
  // Equal buys have no spread to spend, so they get no heights at all rather than a row of
  // maximums claiming every one of them was the biggest. A single buy in view is that same case.
  #sizing(stacks) {
    const buys = stacks.flatMap((stack) => [...stack.assets.values()]);
    if (new Set(buys.map((buy) => buy.symbol)).size !== 1) return null;

    const quotes = buys.map((buy) => buy.quote);
    const min = Math.min(...quotes);
    const max = Math.max(...quotes);
    return max > min ? { min, span: max - min } : null;
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
    // slivers behind it.
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
      const share = (buy.quote - scale.min) / scale.span;
      mark.style.height = `${LOGO_SIZE + share * (MARK_MAX_HEIGHT - LOGO_SIZE)}px`;
    }
    return mark;
  }

  // A coloured disc carrying the asset's logo. The colour is always painted and the image sits
  // over it, so a symbol the venue has no ticker for still gets a disc — in the grey the holdings
  // tables fall back to — rather than the purchase silently going unmarked.
  #disc(symbol, className) {
    const asset = this.buyLogosValue[symbol] || {};
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

  // What to draw for the mode in hand: the datasets, the y scale they need, how many points
  // survive decimation, and (RETURN only) the colour of the 100 line. Falls back to VALUE when
  // there is no return curve to show — a bot that never held anything has no return.
  #plot(labels) {
    const curve = this.#pnlMode ? this.#pnlPoints(labels) : [];
    return curve.length ? this.#pnlPlot(curve) : this.#valuePlot(labels);
  }

  #valuePlot(labels) {
    // Nulls dropped, not zeroed: a focused holding has no market value at a point the portfolio
    // left on its fill mark. Every point carries its own timestamp, so a gap costs nothing.
    const series = this.#series.map((serie) =>
      serie
        .map((amount, i) => ({ x: labels[i], y: amount === null ? null : Number(amount) }))
        .filter((point) => point.y !== null)
    );
    const maxValue = this.focused
      ? this.#assetBounds.value
      : Math.max(...series[0].map((p) => p.y), ...series[1].map((p) => p.y));
    const profitable = series[0].at(-1).y >= series[1].at(-1).y;
    const points = Math.min(this.maxPointsToDraw, series[0].length, series[1].length);
    const colors = [profitable ? this.#color("--grass") : this.#color("--berry"), this.#color("--benchmark")];

    return {
      points,
      y: { display: false, min: 0, suggestedMax: maxValue * 1.1 },
      datasets: colors.map((color, i) => ({
        ...this.#lineDataset(color, series[i], points),
        // Invested is a step function — the cash lands at the moment of a buy and sits flat
        // until the next one — so it gets right-angle risers instead of a curve. "after":
        // the riser is drawn at the transaction that raised the total, not one point early.
        stepped: i === 1 ? "after" : false,
      })),
    };
  }

  // The invested line becomes the zero line, so the curve IS the distance from it, and the
  // fill is the reading: the ribbon between the curve and zero, green above and red below.
  #pnlPlot(curve) {
    const ys = curve.map((point) => point.y);
    // Zero stays in frame whether or not the curve reaches it: the line is what the curve means.
    const low = this.focused ? this.#assetBounds.low : Math.min(0, ...ys);
    const high = this.focused ? this.#assetBounds.high : Math.max(0, ...ys);
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
        ...this.#lineDataset(curve.at(-1).y >= 0 ? up : down, curve, points),
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
    return this.pnlSeries
      .map((point, i) => ({ x: labels[i], y: point === null ? null : Number(point) }))
      .filter((point) => point.y !== null);
  }

  // Shared shape of every line here: the curve, and a permanent dot on its last point that
  // swaps fill on hover instead of resizing. Decimation samples down to `points` and always
  // keeps the last one, so the radius array is sized to match.
  #lineDataset(color, data, points) {
    return {
      cubicInterpolationMode: "monotone",
      borderWidth: 2,
      borderColor: color,
      pointRadius: Array(points - 1).fill(0).concat([3]),
      pointHitRadius: 0,
      pointBackgroundColor: color,
      pointBorderColor: color,
      pointBorderWidth: 0,
      pointHoverRadius: 3,
      pointHoverBackgroundColor: this.#color("--washed"),
      pointHoverBorderColor: color,
      pointHoverBorderWidth: 2,
      data,
      clip: false,
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
