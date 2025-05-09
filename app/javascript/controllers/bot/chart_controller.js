import { Controller } from "@hotwired/stimulus";
import { Chart, Tooltip } from "chart.js/auto";
import "chartjs-adapter-date-fns";
window.Chart = Chart;
window.Tooltip = Tooltip;

// Connects to data-controller="bot--chart"
export default class extends Controller {
  static targets = ["analyzerChart"];
  static values = { series: Array, labels: Array, names: Array, colors: Array };

  connect() {
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
  }

  #buildChart() {
    const all_names = this.namesValue;
    const all_series = this.seriesValue;
    const all_labels = this.labelsValue;

    let minValue;
    let maxValue;
    const profitable = all_series[0][0][all_series[0][0].length - 1] > all_series[0][0][0];
    const success_color = this.#safeColor(this.#getCssVariableValue("--success"));
    const danger_color = this.#safeColor(this.#getCssVariableValue("--danger"));
    const portfolio_color = profitable ? success_color : danger_color;
    const benchmark_color = this.#safeColor(this.#getCssVariableValue("--benchmark"));
    const font_color = this.#safeColor(this.#getCssVariableValue("--label"));
    const tooltip_background_color = this.#safeColor(this.#getCssVariableValue("--tooltip-background"));
    const tooltip_font_color = this.#safeColor(this.#getCssVariableValue("--tooltip-text"));
    const maxPointsToDraw = Math.min(
      this.maxPointsToDraw,
      all_series[0][0].length,
      all_series[0][1].length
    );

    let log_scale = false;

    let series = all_series[0];
    let labels;
    let pointRadius;
    labels = all_labels[0].map((date) => new Date(date).getTime());
    series[0] = series[0].map((value) => Number(value));
    series[0] = series[0].map((x, j) => ({ x: labels[j], y: x }));
    minValue = Math.min(minValue, ...series[0].map((x) => x.y));
    maxValue = Math.max(maxValue, ...series[0].map((x) => x.y));
    series[1] = series[1].map((value) => Number(value));
    series[1] = series[1].map((x, j) => ({ x: labels[j], y: x }));
    minValue = Math.min(minValue, ...series[1].map((x) => x.y));
    maxValue = Math.max(maxValue, ...series[1].map((x) => x.y));
    pointRadius = 4;

    const datasets = [
      {
        label: all_names[0],
        lineTension: 0,
        borderWidth: 2.5,
        borderColor: portfolio_color,
        pointRadius: Array(maxPointsToDraw - 1)
          .fill(0)
          .concat([pointRadius]),
        pointHoverRadius: Array(maxPointsToDraw - 1)
          .fill(0)
          .concat([pointRadius]),
        pointHitRadius: 0,
        pointBackgroundColor: portfolio_color,
        pointBorderColor: this.#setTransparency(portfolio_color, 0.5),
        pointBorderWidth: 0,
        data: series[0],
        clip: { left: false, top: false, right: false, bottom: false },
      },
      {
        label: all_names[1],
        lineTension: 0,
        borderWidth: 2.5,
        borderColor: benchmark_color,
        pointRadius: Array(maxPointsToDraw - 1)
          .fill(0)
          .concat([3.5]),
        pointHoverRadius: Array(maxPointsToDraw - 1)
          .fill(0)
          .concat([3.5]),
        pointHitRadius: 0,
        pointBackgroundColor: benchmark_color,
        pointBorderColor: this.#setTransparency(benchmark_color, 0.5),
        pointBorderWidth: 0,
        data: series[1],
        clip: { left: false, top: false, right: false, bottom: false },
      },
    ];

    Tooltip.positioners.topLeft = function (elements, eventPosition) {
      const tooltip = this;
      return { x: 5, y: -5 };
    };

    Tooltip.positioners.dynamicPosition = function (elements, eventPosition) {
      const tooltip = this;
      const chartArea = tooltip.chart.chartArea;
      const cursorX = eventPosition.x;

      let y = -10;
      let x = 10;

      if (cursorX < tooltip.width + x) {
        x = chartArea.width;
      }

      return { x: x, y: y };
    };

    Chart.defaults.font.family = "Montserrat";
    if (this.chart) {
      this.chart.destroy();
    }
    this.chart = new Chart(this.#canvasContext(), {
      type: "line",
      plugins: [
        {
          afterDatasetsDraw: (chart) => {
            if (chart.tooltip?._active?.length) {
              let x = chart.tooltip._active[0].element.x;
              let yAxis = chart.scales.y;
              let ctx = chart.ctx;
              ctx.save();
              ctx.beginPath();
              ctx.moveTo(x, yAxis.top);
              ctx.lineTo(x, yAxis.bottom);
              ctx.lineWidth = 0.5;
              ctx.strokeStyle = font_color;
              ctx.stroke();
              ctx.restore();
            }
          },
          afterDraw: (chart) => {
            // Trigger the tooltip to be redrawn, to avoid the line being drawn over it
            if (chart.tooltip?._active?.length) {
              chart.tooltip.update();
              chart.tooltip.draw(chart.ctx);
            }
          },
        },
      ],
      data: {
        datasets: datasets,
      },
      options: {
        responsive: true,
        maintainAspectRatio: true,
        animation: {
          numbers: { duration: 0 },
        },
        scales: {
          x: {
            type: "time",
            time: {
              unit: "day",
              tooltipFormat: "yyyy/MM/dd",
            },
            ticks: {
              display: false,
            },
            grid: {
              display: false,
            },
            border: {
              display: false,
            },
          },
          y: {
            type: log_scale ? "logarithmic" : "linear",
            min: minValue,
            max: maxValue,
            beginAtZero: false,
            scaleLabel: {
              display: false,
            },
            ticks: {
              display: false,
            },
            grid: {
              display: false,
            },
            border: {
              display: false,
            },
          },
        },
        plugins: {
          legend: {
            display: false,
          },
          tooltip: {
            intersect: false,
            boxPadding: 5,
            usePointStyle: true,
            padding: 16,
            cornerRadius: 5,
            caretPadding: 13,
            caretSize: 0,
            titleColor: tooltip_font_color,
            titleFont: { size: 16 },
            bodyColor: tooltip_font_color,
            bodyFont: { size: 16, weight: 550 },
            bodySpacing: 5,
            backgroundColor: tooltip_background_color,
            position: "dynamicPosition",
            boxWidth: 16,
            callbacks: {
              label: function (context) {
                return `${context.dataset.label}: ${context.parsed.y}`;
              },
            },
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
