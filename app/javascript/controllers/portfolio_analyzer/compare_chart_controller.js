import { Controller } from "@hotwired/stimulus";
import { Chart, Tooltip } from "chart.js/auto";
import "chartjs-adapter-date-fns";
window.Chart = Chart;
window.Tooltip = Tooltip;

export default class extends Controller {
  static targets = ["compareChart"];
  static values = { series: Array, labels: Array, names: Array };

  connect() {
    console.log('chart compare ok')
    this.resizeObserver = new ResizeObserver(() => {
      const width = this.element.offsetWidth;
      if (width !== this.previousWidth) {
        this.maxPointsToDraw = Math.floor(width / 3.5);
        this.previousWidth = width;
        this.#buildChart();
      }
    });
    this.resizeObserver.observe(this.element);
  }

  disconnect() {
    this.resizeObserver.disconnect();
  }

  #buildChart() {
    const all_names = this.namesValue;
    let all_series = this.seriesValue;
    const all_labels = this.labelsValue;

    // 0. Find common dates between all series
    const commonDates = all_labels.reduce((acc, arr) => acc.filter(date => arr.includes(date)), all_labels[0]);
    const labels = commonDates.map(date => new Date(date).getTime());
    console.log(commonDates)
    console.log(labels)

    // 1. Remove benchmark series
    all_series = all_series.map(series => series[0]);
    console.log(all_series)

    // 2. Filter out dates that are not common to all series
    all_series = all_series.map((arr, index) =>
      arr.filter((_, i) => commonDates.includes(all_labels[index][i]))
    );
    console.log(all_series)

    // Normalize each series to start at 0
    all_series = all_series.map(arr => {
      const firstValue = arr[0];
      return arr.map(value => (value / firstValue));
    });

    // 3. Map the dates to the x-axis
    all_series = all_series.map((series, index) =>
      series.map((x, i) => ({ x: labels[i], y: x }))
    );
    console.log(all_series)

    const allYValues = all_series.flatMap(subArray => subArray.map(obj => obj.y));
    const minValue = Math.min(...allYValues);
    const maxValue = Math.max(...allYValues);
    const font_color = this.#safeColor(this.#getCssVariableValue('--label'));
    const tooltip_background_color = this.#safeColor(this.#getCssVariableValue('--tooltip-background'));
    const tooltip_font_color = this.#safeColor(this.#getCssVariableValue('--tooltip-text'));
    const maxPointsToDraw = Math.min(this.maxPointsToDraw, labels.length);
    const getColor = this.#createColorGenerator();

    let log_scale = true;

    let datasets = [];
    for (let i = 0; i < all_names.length; i++) {
      const portfolio_color = this.#safeColor(getColor()) // this.#safeColor(this.#getCssVariableValue('--success'))
      const portfolio_gradient = this.#canvasContext().createLinearGradient(0, 0, 0, 300);
            portfolio_gradient.addColorStop(0, this.#setTransparency(portfolio_color, 0.2));
            portfolio_gradient.addColorStop(1, this.#setTransparency(portfolio_color, 0));
      datasets.push({
        label: all_names[i],
        lineTension: 0,
        borderWidth: 2.5,
        borderColor: portfolio_color,
        pointRadius: Array(maxPointsToDraw - 1)
          .fill(0)
          .concat([4]),
        pointHoverRadius: Array(maxPointsToDraw - 1)
          .fill(0)
          .concat([4]),
        pointHitRadius: 0,
        pointBackgroundColor: portfolio_color,
        pointBorderColor: this.#setTransparency(portfolio_color, 0.5),
        pointBorderWidth: 0,
        data: all_series[i],
        clip: {left: false, top: false, right: false, bottom: false},
      });
    }

    Tooltip.positioners.topLeft = function(elements, eventPosition) {
        const tooltip = this;
        return { x: 5, y: -5 };
    };

    Tooltip.positioners.dynamicPosition = function(elements, eventPosition) {
      const tooltip = this;
      const chartArea = tooltip.chart.chartArea;
      const cursorX = eventPosition.x;

      let y = -10
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
              font: { size: 11 },
              fontColor: font_color,
              autoSkip: true,
              maxTicksLimit: 5,
              maxRotation: 0,
              minRotation: 0,
            },
            grid: {
              display: false,
              lineWidth: 0.5,
              color: font_color,
              drawOnChartArea: false,
              zeroLineWidth: 1,
            },
            border: {
              display: false,
            },
          },
          y: {
            type: "logarithmic",
            min: minValue,
            max: maxValue,
            beginAtZero: false,
            scaleLabel: {
              display: false,
              labelString: "USD",
              fontColor: font_color,
            },
            ticks: {
              display: false,
              font: { size: 11 },
              fontColor: font_color,
              beginAtZero: true,
              autoSkip: false,
              callback: function (value, index, values) {
                if (log_scale) {
                  if (
                    value == 1 ||
                    value == 1e1 ||
                    value == 1e2 ||
                    value == 1e3 ||
                    value == 1e4 ||
                    value == 1e5 ||
                    value == 1e6 ||
                    value == 1e7 ||
                    value == 1e8 ||
                    value == 1e9 ||
                    value == 1e10 ||
                    value == 1e11 ||
                    value == 1e12 ||
                    value == 3.5 ||
                    value == 3.5e1 ||
                    value == 3e2 ||
                    value == 3e3 ||
                    value == 3e4 ||
                    value == 3e5 ||
                    value == 3e6 ||
                    value == 3e7 ||
                    value == 3e8 ||
                    value == 3e9 ||
                    value == 3e10 ||
                    value == 3e11 ||
                    value == 3e12
                  ) {
                    return new Intl.NumberFormat("en-US", {
                      style: "currency",
                      currency: "USD",
                      minimumFractionDigits: 0,
                    }).format(value);
                  }
                } else {
                  return "$" + value + " ";
                }
              },
            },
            grid: {
              display: false,
              lineWidth: 1,
              zeroLineWidth: 1,
            },
            border: {
              display: false,
            },
          },
        },
        plugins: {
          legend: {
            display: true,
            position: "bottom",
            align: "center",
            labels: {
              font: { size: 16 },
              fontColor: font_color,
              usePointStyle: true,
              boxWidth: 5,
              boxHeight: 5,
              padding: 25,
            },
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
                let label = context.dataset.label + ": ";
                if (context.parsed.y !== null) {
                  label += new Intl.NumberFormat("en-US", {
                    style: "currency",
                    currency: "USD",
                  }).format(context.parsed.y);
                }
                return label;
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
    return this.compareChartTarget.getContext("2d", { colorSpace: "display-p3" });
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
    return this.#displaySupportsP3Color() && this.#canvasSupportsDisplayP3() && this.#canvasSupportsWideGamutCSSColors();
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
    if (this.#isValidDisplayP3Color(color) && !this.#wideGamutColorSupported()) {
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
      throw new Error('Invalid color(display-p3 ...) string');
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
      throw new Error('Invalid hex color code');
    }
    hex = hex.slice(1);
    let r, g, b, a = 255;
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
      throw new Error('Invalid rgb color code');
    }
    const match = rgbString.match(this.#rgbRegex());
    if (match) {
      const r = match[1];
      const g = match[2];
      const b = match[3];
      const a = 1;
      return `rgba(${r}, ${g}, ${b}, ${a})`;
    }
    throw new Error('Unexpected error parsing the RGB string');
  }

  #setTransparency(color, transparency) {
    // Validate transparency value
    if (transparency < 0 || transparency > 1) {
      throw new Error('Transparency value must be between 0 and 1');
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

    throw new Error('Invalid color format. Must be display-p3 or rgba.');
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

  #createColorGenerator() {
    const colors = ['#FF5733', '#33FF57', '#3357FF', '#FF33A1', '#A133FF']; // List of 5 colors
    let currentIndex = 0; // Tracks the current color index

    return function() {
        const color = colors[currentIndex]; // Get the current color
        currentIndex = (currentIndex + 1) % colors.length; // Move to the next color, reset if at the end
        return color;
    };
  }
}
