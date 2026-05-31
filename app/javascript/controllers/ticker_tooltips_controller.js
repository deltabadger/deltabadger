import { Controller } from "@hotwired/stimulus";

// Global hover-card for EVERY .ticker pill. Attached once on <body>; uses event delegation
// so it works on any ticker anywhere (including Turbo-rendered content) with no per-view
// wiring. A pill only carries its symbol text, so the server resolves that to an asset and
// returns the card HTML. One shared popover element (top layer) is reused for all pills —
// it escapes the pill's overflow:hidden and any transformed/clipping ancestor (e.g. the
// asset-picker modal). Card data (logo/name/symbol/type/price) comes from whatever market
// data provider is configured (data API or CoinGecko), so it needs no gating.
const CARD_CACHE = new Map();

export default class extends Controller {
  connect() {
    this.tooltip = this.#buildTooltip();
    this.onOver = this.#onOver.bind(this);
    this.onOut = this.#onOut.bind(this);
    this.element.addEventListener("mouseover", this.onOver);
    this.element.addEventListener("mouseout", this.onOut);
  }

  disconnect() {
    this.element.removeEventListener("mouseover", this.onOver);
    this.element.removeEventListener("mouseout", this.onOut);
    this.#hide();
    this.tooltip?.remove();
  }

  #onOver(event) {
    const pill = event.target.closest(".ticker");
    if (!pill || !this.#eligible(pill) || pill === this.current) return;

    this.current = pill;
    this.#reveal(pill);
  }

  #onOut(event) {
    const pill = event.target.closest(".ticker");
    if (!pill) return;
    // Ignore moves between children of the same pill.
    if (event.relatedTarget && pill.contains(event.relatedTarget)) return;
    this.current = null;
    this.#hide();
  }

  #reveal(pill) {
    const symbol = this.#symbolFor(pill);
    if (!symbol) return;

    this.#loadCard(symbol).then((html) => {
      // Bail if the cursor already moved on to a different pill while fetching.
      if (!html || this.current !== pill) return;
      this.tooltip.innerHTML = html;
      this.#appendNote(pill);
      this.#show(pill);
    });
  }

  // Skip pills that aren't a resolvable asset: the "+N" badge, EDITABLE controls (search/amount
  // fields), and empties. A readonly input (e.g. the wizard's filled chip) IS an asset display.
  #eligible(pill) {
    if (pill.classList.contains("ticker--more")) return false;
    if (pill.querySelector("input:not([readonly]), select, textarea")) return false;
    return this.#symbolFor(pill).length > 0;
  }

  // The symbol may live in the pill's text or in a readonly input's value (wizard chips).
  #symbolFor(pill) {
    return (pill.dataset.tickerSymbol || pill.textContent || pill.querySelector("input")?.value || "").trim();
  }

  #loadCard(symbol) {
    const key = symbol.toUpperCase();
    if (CARD_CACHE.has(key)) return Promise.resolve(CARD_CACHE.get(key));

    const url = `${this.#basePath}/asset_tooltip?symbol=${encodeURIComponent(symbol)}`;
    return fetch(url, { headers: { Accept: "text/html", "X-CSRF-Token": this.#csrfToken } })
      .then((response) => (response.ok ? response.text() : ""))
      .then((html) => {
        CARD_CACHE.set(key, html);
        return html;
      })
      .catch(() => "");
  }

  // Pills that carry a `data-ticker-note` (e.g. index pie slices) append an extra
  // live line to the cached card — the per-symbol cache stays generic, the note doesn't.
  #appendNote(pill) {
    const note = pill.dataset.tickerNote;
    if (!note) return;
    const el = document.createElement("div");
    el.className = "tooltip--ticker__allocation";
    el.textContent = note;
    // Append inside the info column (under name/symbol) so it stays within the
    // card's width — the name column is a fixed 20rem and the popover caps at 30rem,
    // so a separate right-aligned item would overflow off the card.
    const col = this.tooltip.querySelector(".tooltip--ticker__col:last-of-type");
    (col || this.tooltip).appendChild(el);
  }

  #show(pill) {
    if (typeof this.tooltip.showPopover === "function") {
      if (!this.tooltip.matches(":popover-open")) this.tooltip.showPopover();
    } else {
      this.tooltip.classList.add("is-open");
    }
    this.#position(pill);
  }

  #hide() {
    if (typeof this.tooltip?.hidePopover === "function") {
      if (this.tooltip.matches(":popover-open")) this.tooltip.hidePopover();
    } else {
      this.tooltip?.classList.remove("is-open");
    }
  }

  // Pin below the pill, clamped to the viewport. The shared popover lives in the top layer,
  // so these viewport coordinates are correct even inside transformed/overflow ancestors.
  #position(pill) {
    const tip = this.tooltip;
    const anchor = pill.getBoundingClientRect();
    const margin = 8;

    tip.style.position = "fixed";
    tip.style.right = "auto";
    tip.style.bottom = "auto";
    tip.style.left = "0px";
    tip.style.top = "0px";

    const rect = tip.getBoundingClientRect();
    let left = anchor.left; // align the card's left edge with the pill's left edge
    if (left + rect.width > window.innerWidth - margin) left = window.innerWidth - margin - rect.width;
    if (left < margin) left = margin;

    tip.style.left = `${left}px`;
    tip.style.top = `${anchor.bottom + 6}px`;
  }

  #buildTooltip() {
    const el = document.createElement("div");
    el.className = "tooltip tooltip--ticker";
    el.setAttribute("popover", "manual");
    document.body.appendChild(el);
    return el;
  }

  // Locale prefix (e.g. "/pl") so the fetch stays inside the authenticated locale scope.
  get #basePath() {
    const match = window.location.pathname.match(/^\/[a-z]{2}(?=\/|$)/);
    return match ? match[0] : "";
  }

  get #csrfToken() {
    return document.querySelector('meta[name="csrf-token"]')?.content;
  }
}
