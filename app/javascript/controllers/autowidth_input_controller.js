import { Controller } from "@hotwired/stimulus"

// Automatically adjusts the width of an input element based on its content length using a mirror element for precise calculation.
export default class extends Controller {

  connect() {
    this.resize = this.resize.bind(this)
    this.#createMirrorElement()
    this.#transferStyles()
    requestAnimationFrame(this.resize)
    this.element.addEventListener('input', this.resize)
  }

  disconnect() {
    this.element.removeEventListener('input', this.resize)
  }

  resize() {
    // Use input value or placeholder as the text to measure
    const text = this.element.value || this.element.placeholder || ''
    this.mirrorElement.textContent = text;

    // Measure the width of the mirror element
    const measuredWidth = this.mirrorElement.offsetWidth;

    // Add a small buffer (e.g., 2px) to prevent text clipping or overflow
    const buffer = 2;
    const newWidth = measuredWidth + buffer;

    // Set the input element's width
    this.element.style.width = `${newWidth}px`;
  }

  #createMirrorElement() {
    this.mirrorElement = document.createElement('span')
    // Apply styles to make it invisible but measurable
    Object.assign(this.mirrorElement.style, {
      position: 'absolute',
      visibility: 'hidden',
      height: 'auto',
      width: 'auto',
      whiteSpace: 'pre', // Use 'pre' to respect spaces and prevent wrapping
      top: '-9999px',      // Position off-screen
      left: '-9999px'
    })
    document.body.appendChild(this.mirrorElement)
  }

  #transferStyles() {
    const computedStyle = window.getComputedStyle(this.element)
    // List of CSS properties that affect horizontal size
    const stylesToCopy = [
      'fontSize', 'fontFamily', 'fontWeight', 'fontStyle',
      'letterSpacing', 'textTransform', 'wordSpacing',
      'paddingLeft', 'paddingRight',
      'borderLeftWidth', 'borderRightWidth',
      // Add any other relevant style properties if needed
    ];

    stylesToCopy.forEach(prop => {
      this.mirrorElement.style[prop] = computedStyle[prop]
    })
  }
}