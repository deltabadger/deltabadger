import { Controller } from "@hotwired/stimulus";
import Chart from "chart.js/auto";
import "chartjs-adapter-date-fns";

// Connects to data-controller="bot--chart"
export default class extends Controller {
  static targets = ["analyzerChart", "summary", "date", "pnl", "percent"];
  static values = {
    series: Array,
    labels: Array,
    quote: String,
    decimals: Number,
    bot: Number,
    return: Array,
    returnNote: String,
  };

  connect() {
    // The whole chart is broadcast-replaced every metrics cycle (~5 min), which destroys this
    // controller and re-renders the switch on VALUE. Without somewhere outside the element to
    // remember it, a user reading RETURN gets thrown back to VALUE while looking at it.
    this.currentMode = this.#storedMode();
    // Seeded synchronously: the ResizeObserver below is debounced by 100ms, and a mode click
    // inside that window would otherwise rebuild with an undefined budget (Array(NaN) throws,
    // leaving the buttons in one mode and the summary in the other).
    this.maxPointsToDraw = Math.max(1, Math.floor(this.element.offsetWidth / 3.5));
    this.#renderSummary();
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

    // After the child control has connected, so the chip moves with it.
    if (this.#returnMode) {
      requestAnimationFrame(() => this.element.querySelector('[data-value="return"]')?.click());
    }
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

  // --- VALUE / RETURN switch -----------------------------------------------------------
  //
  // The control itself (chip, aria, arrow keys) is the `segmented` controller's; this only
  // reacts to the choice. Both curves are already here, so a mode change is a redraw and
  // never a fetch.
  //
  // NOT this.mode = …: assigning that would shadow this action method.
  mode(event) {
    this.currentMode = event.detail.value;
    this.#remember(this.currentMode);
    this.#buildChart();
    this.#renderSummary();
  }

  // Per tab, not per browser: a mode is a way of reading this page now, not a preference.
  get #storageKey() {
    return `bot-chart-mode:${this.botValue}`;
  }

  #storedMode() {
    try {
      return sessionStorage.getItem(this.#storageKey) === "return" ? "return" : "value";
    } catch {
      return "value"; // storage can be denied outright (private mode, embedded webview)
    }
  }

  #remember(mode) {
    try {
      sessionStorage.setItem(this.#storageKey, mode);
    } catch {
      // Not being able to remember the mode is not a reason to fail switching it.
    }
  }

  // Requires an actual curve: the plot falls back to VALUE when there is none, and the summary
  // has to fall back with it or the page would caption a value curve as a return.
  get #returnMode() {
    return this.currentMode === "return" && this.returnValue?.some((index) => index !== null);
  }

  // --- summary above the chart: date, PnL in quote currency, PnL in % ------------------

  get #locale() {
    return document.documentElement.lang || "en";
  }

  #timestamps() {
    this.stamps ||= this.labelsValue.map((date) => new Date(date).getTime());
    return this.stamps;
  }

  #date(timestamp) {
    return new Date(timestamp).toLocaleDateString(this.#locale, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }

  #money(value) {
    const decimals = Math.abs(value) >= 1 ? 2 : this.decimalsValue;
    return `${value.toLocaleString(this.#locale, {
      minimumFractionDigits: Math.min(2, decimals),
      maximumFractionDigits: decimals,
      signDisplay: "exceptZero",
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
  #renderSummary(point = null) {
    this.summaryTarget.classList.toggle("widget--chart__summary--return", this.#returnMode);
    if (this.#returnMode) return this.#renderReturnSummary(point);

    const value = this.seriesValue[0];
    const invested = this.seriesValue[1];
    if (!value?.length) return;

    const timestamps = this.#timestamps();
    const last = value.length - 1;
    const at = point || { x: timestamps[last], value: value[last], invested: invested[last] };
    const spent = Number(at.invested);
    const pnl = Number(at.value) - spent;
    const first = this.#date(timestamps[0]);
    const on = this.#date(at.x);

    this.dateTarget.textContent = point || first === on ? on : `${first} – ${on}`;
    this.pnlTarget.textContent = this.#money(pnl);
    this.percentTarget.textContent = this.#percent(spent > 0 ? pnl / spent : 0);
    this.summaryTarget.classList.toggle("text-danger", pnl < 0);
    this.summaryTarget.classList.toggle("text-success", pnl >= 0);
  }

  // RETURN carries no money: the index is a ratio, so the big line is the growth it stands for
  // and the small line captions the mode instead of printing the same number a second time.
  #renderReturnSummary(point = null) {
    const curve = this.returnValue;
    if (!curve?.length) return;

    const timestamps = this.#timestamps();
    const last = curve.length - 1;
    const at = point || { x: timestamps[last], index: curve[last] };
    if (at.index === null || at.index === undefined) return;

    const growth = Number(at.index) / 100 - 1;
    const first = this.#date(timestamps[0]);
    const on = this.#date(at.x);

    this.dateTarget.textContent = point || first === on ? on : `${first} – ${on}`;
    this.pnlTarget.textContent = this.#percent(growth);
    this.percentTarget.textContent = this.returnNoteValue;
    this.summaryTarget.classList.toggle("text-danger", growth < 0);
    this.summaryTarget.classList.toggle("text-success", growth >= 0);
  }

  #updateSummary(tooltip) {
    if (!tooltip.getActiveElements().length) return this.#renderSummary();

    const points = tooltip.dataPoints;
    if (this.#returnMode) {
      const parsed = points[0]?.parsed;
      return parsed && this.#renderSummary({ x: parsed.x, index: parsed.y });
    }

    const value = points.find((p) => p.datasetIndex === 0)?.parsed;
    if (!value) return;

    // Read invested from the source series at the hovered timestamp, NOT from the other
    // dataset's point at the same index: LTTB decimates each dataset on its own triangle
    // areas, so past `maxPointsToDraw` the two datasets no longer sample the same points and
    // index-mode would pair a value with an invested from a different day.
    this.#renderSummary({ x: value.x, value: value.y, invested: this.#investedAt(value.x) });
  }

  // The invested total in force at a timestamp: the last transaction at or before it. Binary
  // search — the labels are sorted and this runs on every pointer move.
  #investedAt(timestamp) {
    const timestamps = this.#timestamps();
    let low = 0;
    let high = timestamps.length - 1;
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if (timestamps[middle] <= timestamp) low = middle;
      else high = middle - 1;
    }
    return Number(this.seriesValue[1][low]);
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
          // The 100 line the return curve is read against, under the datasets so the curve and
          // its ribbon sit on top of it. VALUE has no baseline — its own capital curve is one.
          beforeDatasetsDraw: (chart) => {
            if (!plot.baseline) return;

            const y = chart.scales.y.getPixelForValue(100);
            const ctx = chart.ctx;
            ctx.save();
            ctx.beginPath();
            ctx.setLineDash([3, 3]);
            ctx.moveTo(chart.chartArea.left, y);
            ctx.lineTo(chart.chartArea.right, y);
            ctx.lineWidth = 1;
            ctx.strokeStyle = plot.baseline;
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
  }

  // What to draw for the mode in hand: the datasets, the y scale they need, how many points
  // survive decimation, and (RETURN only) the colour of the 100 line. Falls back to VALUE when
  // there is no return curve to show — a bot that never held anything has no return.
  #plot(labels) {
    const curve = this.#returnMode ? this.#returnPoints(labels) : [];
    return curve.length ? this.#returnPlot(curve) : this.#valuePlot(labels);
  }

  #valuePlot(labels) {
    const series = this.seriesValue.map((serie) =>
      serie.map((amount, i) => ({ x: labels[i], y: Number(amount) }))
    );
    const maxValue = Math.max(...series[0].map((p) => p.y), ...series[1].map((p) => p.y));
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

  // No block of committed capital to sit on, so the fill IS the reading: the ribbon between the
  // curve and its 100 line, green above and red below, which is the only thing separating a
  // winning window from a losing one.
  #returnPlot(curve) {
    const ys = curve.map((point) => point.y);
    // 100 stays in frame whether or not the curve reaches it: the line is what the curve means.
    const low = Math.min(100, ...ys);
    const high = Math.max(100, ...ys);
    const pad = (high - low) * 0.1 || 1;
    const color = curve.at(-1).y >= 100 ? this.#color("--grass") : this.#color("--berry");
    const points = Math.min(this.maxPointsToDraw, curve.length);

    return {
      points,
      baseline: this.#color("--benchmark"),
      y: { display: false, min: low - pad, max: high + pad },
      datasets: [{
        ...this.#lineDataset(color, curve, points),
        fill: {
          value: 100,
          above: this.#setTransparency(this.#color("--grass"), 0.12),
          below: this.#setTransparency(this.#color("--berry"), 0.12),
        },
      }],
    };
  }

  // Points from before the bot held anything arrive as nulls, so the curve stays aligned with
  // the labels. They are dropped here — every point carries its own timestamp, and decimation
  // cannot sample around a hole.
  #returnPoints(labels) {
    return this.returnValue
      .map((index, i) => ({ x: labels[i], y: index === null ? null : Number(index) }))
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
