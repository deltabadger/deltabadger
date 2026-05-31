import { Controller } from "@hotwired/stimulus";
import { renderDonut } from "../lib/donut_chart";

// Connects to data-controller="index-allocation"
// Updates index asset allocations in real-time when sliders change, and toggles
// between the bar list and a donut/pie view (preference remembered per bot).
export default class extends Controller {
  static targets = ["flattening", "flatteningValue", "flatteningTrack", "numCoins", "numCoinsValue", "numCoinsTrack", "assets", "note", "list", "pie", "pieSvg"];
  static values = { botId: String, otherLabel: String };

  connect() {
    this.cacheMarketCaps();
    this.updateAll();
    // Targets only exist when the preview rendered (block is gated on index_preview).
    if (this.hasListTarget && this.hasPieTarget) this.applyView(this.#storedView());
  }

  // ----- View toggle: clicking either visualization flips it (pie default,
  // remembered per bot in localStorage). No dedicated toggle buttons. -----

  toggle(event) {
    event?.preventDefault();
    const next = this.view === "pie" ? "list" : "pie";
    this.applyView(next);
    this.#persistView(next);
  }

  applyView(view) {
    if (!this.hasListTarget || !this.hasPieTarget) return;
    this.view = view === "list" ? "list" : "pie";
    const pie = this.view === "pie";

    this.pieTarget.classList.toggle("is-hidden", !pie);
    this.listTarget.classList.toggle("is-hidden", pie);

    if (pie) this.renderPie();
  }

  renderPie() {
    if (!this.hasPieSvgTarget || !this.slices || this.slices.length === 0) return;
    // labelColor: currentColor so slice names follow the theme text color (the lib's
    // #1a1a1a default vanishes on the dark widget background). `%`/leaders stay muted grey.
    // An index can hold up to 50 coins; the donut still draws every slice (all hoverable),
    // but the legend collapses everything below 4% of the allocation into one "Other" entry.
    renderDonut(this.pieSvgTarget, this.slices, {
      showLabels: true,
      tooltips: true,
      labelColor: "currentColor",
      otherThreshold: 0.04,
      otherLabel: this.hasOtherLabelValue ? this.otherLabelValue : "Other"
    });
  }

  #storedView() {
    if (!this.botIdValue) return "pie";
    try {
      return localStorage.getItem(`index-view:${this.botIdValue}`) || "pie";
    } catch {
      return "pie";
    }
  }

  #persistView(view) {
    if (!this.botIdValue) return;
    try {
      localStorage.setItem(`index-view:${this.botIdValue}`, view);
    } catch {
      // storage unavailable/blocked — preference simply isn't remembered
    }
  }

  cacheMarketCaps() {
    if (!this.hasAssetsTarget) return;

    this.allAssets = this.assetsTarget.querySelectorAll('.index-asset');
    this.marketCaps = [];
    this.allAssets.forEach((asset) => {
      this.marketCaps.push(parseFloat(asset.dataset.marketCap) || 0);
    });
    this.totalAvailable = parseInt(this.assetsTarget.dataset.totalAvailable) || this.allAssets.length;
  }

  updateFlattening(event) {
    const value = parseFloat(event.target.value) || 0;
    const percentage = value * 100;

    // Update slider track grid
    if (this.hasFlatteningTrackTarget) {
      this.flatteningTrackTarget.style.gridTemplateColumns = `${percentage}% auto`;
    }

    // Update value display
    if (this.hasFlatteningValueTarget) {
      this.flatteningValueTarget.textContent = `${Math.round(percentage)}%`;
    }

    // Update asset allocations
    this.updateAllocations();
  }

  updateNumCoins(event) {
    const value = parseInt(event.target.value) || 10;
    const min = parseInt(event.target.min) || 2;
    const max = parseInt(event.target.max) || 50;
    const percentage = ((value - min) / (max - min)) * 100;

    // Update slider track grid
    if (this.hasNumCoinsTrackTarget) {
      this.numCoinsTrackTarget.style.gridTemplateColumns = `${percentage}% auto`;
    }

    // Update value display
    if (this.hasNumCoinsValueTarget) {
      this.numCoinsValueTarget.textContent = value;
    }

    // Update visibility and allocations
    this.updateVisibility(value);
    this.updateAllocations();
  }

  updateAll() {
    const numCoins = this.hasNumCoinsTarget ? parseInt(this.numCoinsTarget.value) || 10 : 10;
    this.updateVisibility(numCoins);
    this.updateAllocations();
  }

  updateVisibility(numCoins) {
    if (!this.allAssets) return;

    this.allAssets.forEach((asset, index) => {
      asset.style.display = index < numCoins ? '' : 'none';
    });

    // Update "fewer coins available" note
    if (this.hasNoteTarget) {
      if (this.totalAvailable < numCoins) {
        this.noteTarget.style.display = '';
        // Update the note text dynamically
        const noteText = this.noteTarget.textContent;
        // Simple replacement - assumes format "Only X of Y coins..."
        this.noteTarget.textContent = noteText.replace(/\d+ of \d+/, `${this.totalAvailable} of ${numCoins}`);
      } else {
        this.noteTarget.style.display = 'none';
      }
    }
  }

  updateAllocations() {
    if (!this.allAssets || !this.marketCaps) return;

    const numCoins = this.hasNumCoinsTarget ? parseInt(this.numCoinsTarget.value) || 10 : 10;
    const flattening = this.hasFlatteningTarget ? parseFloat(this.flatteningTarget.value) || 0 : 0;

    // Calculate total market cap for visible coins only
    const visibleCount = Math.min(numCoins, this.allAssets.length);
    let totalMarketCap = 0;
    for (let i = 0; i < visibleCount; i++) {
      totalMarketCap += this.marketCaps[i];
    }

    const equalWeight = visibleCount > 0 ? 1.0 / visibleCount : 0;

    const slices = [];
    this.allAssets.forEach((asset, index) => {
      if (index >= visibleCount) return;

      const marketCapWeight = totalMarketCap > 0 ? this.marketCaps[index] / totalMarketCap : equalWeight;
      const allocation = marketCapWeight * (1 - flattening) + equalWeight * flattening;
      const allocationPct = (allocation * 100).toFixed(2);

      // Update track grid column
      const track = asset.querySelector('.slider__style__track');
      if (track) {
        track.style.gridTemplateColumns = `${allocationPct}% auto`;
      }

      // Update allocation text
      const allocationText = asset.querySelector('.allocation');
      if (allocationText) {
        allocationText.textContent = `${allocationPct}%`;
      }

      const symbol = (asset.querySelector('.ticker')?.textContent || '').trim();
      slices.push({ symbol, label: symbol, color: asset.dataset.color || '#8A9BA8', value: allocation, logo: asset.dataset.logo || null });
    });

    // Feed the donut view; re-render live when it's the active view.
    this.slices = slices;
    if (this.view === 'pie') this.renderPie();
  }
}
