import { Controller } from "@hotwired/stimulus";
import { Chart, Tooltip } from "chart.js/auto";
import "chartjs-adapter-date-fns";
window.Chart = Chart;
window.Tooltip = Tooltip;

export default class extends Controller {
  static targets = ["analyzerChart"];
  static values = { series: Array, labels: Array };

  connect() {
    const series = this.seriesValue;
    const labels = this.labelsValue.map(date => new Date(date).getTime());
    series[0] = series[0].map((x, i) => ({ x: labels[i], y: x }));
    series[1] = series[1].map((x, i) => ({ x: labels[i], y: x }));
    const profitable = series[0][series[0].length - 1].y > series[0][0].y;
    const rgb_success = this.#hexToRgb(this.#getCssVariableValue('--success'));
    const rgb_danger = this.#hexToRgb(this.#getCssVariableValue('--danger'));
    const portfolio_color = profitable ? rgb_success : rgb_danger
    const benchmark_color = this.#hexToRgb(this.#getCssVariableValue('--primary'));
    const font_color = this.#hexToRgba(this.#getCssVariableValue('--label'));
    const tooltip_background_color = this.#hexToRgb(this.#getCssVariableValue('--widget-background'));
    const portfolio_gradient = this.#canvasContext().createLinearGradient(0, 0, 0, 300);
          portfolio_gradient.addColorStop(0, 'rgba(' + portfolio_color + ', 0.2)');
          portfolio_gradient.addColorStop(1, 'rgba(' + portfolio_color + ', 0)');
    const benchmark_gradient = this.#canvasContext().createLinearGradient(0, 0, 0, 300);
          benchmark_gradient.addColorStop(0, 'rgba(' + benchmark_color + ', 0.2)');
          benchmark_gradient.addColorStop(1, 'rgba(' + benchmark_color + ', 0)');

    let max_points_to_draw = 150;
    let log_scale = true;

    Tooltip.positioners.topLeft = function(elements, eventPosition) {
        const tooltip = this;
        return { x: 5, y: 5 };
    };

    Chart.defaults.font.family = "Montserrat";
    new Chart(this.#canvasContext(), {
      type: "line",
      plugins: [
        {
          afterDraw: (chart) => {
            if (chart.tooltip?._active?.length) {
              let x = chart.tooltip._active[0].element.x;
              let yAxis = chart.scales.y;
              let ctx = chart.ctx;
              ctx.save();
              ctx.beginPath();
              ctx.moveTo(x, yAxis.top);
              ctx.lineTo(x, yAxis.bottom);
              ctx.lineWidth = 0.5;
              ctx.strokeStyle = "#ccc";
              ctx.stroke();
              ctx.restore();
            }
          },
        },
      ],
      data: {
        // labels: labels,
        datasets: [
          {
            label: "Portfolio",
            lineTension: 0, // 0.2,
            borderWidth: 2.5,
            borderColor: 'rgb(' + portfolio_color + ')',
            // backgroundColor: portfolio_gradient,
            // fill: 'origin',
            pointRadius: Array(max_points_to_draw - 1)
              .fill(0)
              .concat([4]),
            pointHoverRadius: 0,
            pointHitRadius: 0,
            pointBackgroundColor: "rgb(" + portfolio_color + ")",
            pointBorderColor: "rgba(" + portfolio_color + ", 0.5)",
            pointBorderWidth: 0,
            data: series[0],
          },
          {
            label: "Benchmark",
            lineTension: 0, // 0.2,
            borderWidth: 2.5,
            borderColor: 'rgb(' + benchmark_color + ')',
            borderDash: [4, 2],
            // backgroundColor: benchmark_gradient,
            // fill: 'origin',
            pointRadius: Array(max_points_to_draw - 1)
              .fill(0)
              // .concat([0]),
              .concat([3.5]),
            pointHoverRadius: 0,
            pointHitRadius: 0,
            pointBackgroundColor: "rgb(" + benchmark_color + ")",
            pointBorderColor: "rgba(" + benchmark_color + ", 0.5)",
            pointBorderWidth: 0,
            data: series[1],
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: true,
        animation: {
          numbers: { duration: 0 },
          // colors: {
          //   type: "color",
          //   duration: 500,
          //   from: "transparent",
          // },
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
              fontColor: "rgba(0, 0, 0, 0.66)",
              autoSkip: true,
              maxTicksLimit: 5,
              maxRotation: 0,
              minRotation: 0,
            },
            grid: {
              display: false,
              lineWidth: 1,
              color: "rgba(0, 0, 0, 0.25)",
              drawOnChartArea: false,
              zeroLineWidth: 1,
            },
            border: {
              display: false,
            },
          },
          y: {
            type: "logarithmic",
            scaleLabel: {
              display: false,
              labelString: "USD",
              fontColor: "rgba(0, 0, 0, 0.66)",
            },
            ticks: {
              display: false,
              font: { size: 11 },
              fontColor: "rgba(0, 0, 0, 0.66)",
              beginAtZero: true,
              // for linear scale
              // autoSkip: true,
              // maxTicksLimit: 6,
              // callback: function(value, index, values) {
              //     return "$" + value + " ";
              // }
              // for log scale
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
              // suggestedMax: Math.max.apply(Math, data[0].y),
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
            display: false,
            position: "bottom",
            align: "center",
            labels: {
              font: { size: 16 },
              fontColor: "rgba(" + font_color + ")",
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
            padding: 10,
            cornerRadius: 5,
            caretPadding: 13,
            caretSize: 0,
            titleColor: "rgba(" + font_color + ")",
            titleFont: { size: 16 },
            bodyColor: "rgba(" + font_color + ")",
            bodyFont: { size: 16 },
            bodySpacing: 5,
            backgroundColor: "transparent",
            // multiKeyBackground: "rgb(" + tooltip_background_color + ")",
            position: "topLeft",
            boxWidth: 10,
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
            samples: max_points_to_draw,
            threshold: max_points_to_draw - 1,
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
    return this.analyzerChartTarget.getContext("2d");
  }

  #hexToRgb(hex) {
    // Ensure the hex code starts with a hash symbol and is of correct length
    if (hex.charAt(0) === '#') {
        hex = hex.substring(1);
    }

    // Check if the hex code is either 3 or 6 characters long
    if (hex.length !== 3 && hex.length !== 6) {
        throw new Error('Invalid hex color code');
    }

    // If the hex code is 3 characters long, convert it to 6 characters
    if (hex.length === 3) {
        hex = hex.split('').map(function (char) {
            return char + char;
        }).join('');
    }

    const r = parseInt(hex.substring(0, 2), 16);
    const g = parseInt(hex.substring(2, 4), 16);
    const b = parseInt(hex.substring(4, 6), 16);

    return `${r}, ${g}, ${b}`;
  }

  #hexToRgba(hex) {
    // Ensure the hex code starts with a hash symbol and is of correct length
    if (hex.charAt(0) === '#') {
        hex = hex.substring(1);
    }

    // Check if the hex code is 8 characters long (including alpha component)
    if (hex.length !== 8) {
        throw new Error('Invalid hex color code');
    }

    // Convert the hex code to RGBA values
    const r = parseInt(hex.substring(0, 2), 16);
    const g = parseInt(hex.substring(2, 4), 16);
    const b = parseInt(hex.substring(4, 6), 16);
    const a = parseInt(hex.substring(6, 8), 16) / 255;

    return `${r}, ${g}, ${b}, ${a.toFixed(2)}`;
  }

  #getCssVariableValue(variableName) {
    const root = document.documentElement;
    const style = getComputedStyle(root);
    const value = style.getPropertyValue(variableName);
    return value.trim();
  }
}
