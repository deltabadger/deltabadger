import { Controller } from "@hotwired/stimulus";

// Holding Shift while dragging a percentage slider coarsens it to whole percent, so a fine-grained
// slider can still be dropped on a round number.
// It lives on the BODY and works off the document: a slider dragged with the mouse never sees the
// Shift key itself, and Turbo streams sliders in long after this connects.
// Only the slider under the pointer is coarsened. A range input rounds its own value to a new step
// the moment that step changes, so reaching its siblings too would rewrite allocations nobody
// dragged — and leave the form refusing to submit the one that was.
// Connects to data-controller="coarse-slider"
const PERCENT_SLIDER = 'input[type="range"][max="100"]';
const COARSE_STEP = "1";

export default class extends Controller {
  #dragged = null;

  connect() {
    document.addEventListener("pointerdown", this.#startDrag);
    document.addEventListener("pointerup", this.#endDrag);
    document.addEventListener("pointercancel", this.#endDrag);
    document.addEventListener("keydown", this.#tune);
    document.addEventListener("keyup", this.#tune);
    // Shift released over another window never reaches this document, and the slider would stay coarse.
    window.addEventListener("blur", this.#endDrag);
  }

  disconnect() {
    document.removeEventListener("pointerdown", this.#startDrag);
    document.removeEventListener("pointerup", this.#endDrag);
    document.removeEventListener("pointercancel", this.#endDrag);
    document.removeEventListener("keydown", this.#tune);
    document.removeEventListener("keyup", this.#tune);
    window.removeEventListener("blur", this.#endDrag);
    this.#endDrag();
  }

  // Shift may already be down when the drag starts, and a held modifier sends no further keydown.
  #startDrag = (event) => {
    if (!event.target.matches?.(PERCENT_SLIDER)) return;

    this.#dragged = event.target;
    if (event.shiftKey) this.#coarsen();
  };

  #endDrag = () => {
    this.#restore();
    this.#dragged = null;
  };

  #tune = (event) => {
    if (event.key !== "Shift" || !this.#dragged) return;

    if (event.type === "keydown") this.#coarsen();
    else this.#restore();
  };

  #coarsen() {
    this.#dragged.dataset.fineStep ??= this.#dragged.step;
    this.#dragged.step = COARSE_STEP;
  }

  #restore() {
    const fineStep = this.#dragged?.dataset.fineStep;
    if (!fineStep) return;

    this.#dragged.step = fineStep;
    delete this.#dragged.dataset.fineStep;
  }
}
