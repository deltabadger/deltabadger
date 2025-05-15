import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="marquee"
export default class extends Controller {
  static targets = ["text"]

  connect() {
    // If text target doesn't exist, disconnect immediately
    if (!this.hasTextTarget) {
      this.disconnect()
      return
    }

    // Build marquee structure once
    this.#setupMarqueeStructure()

    // Observe text changes (e.g., countdown) and container resize
    this.resizeObserver = new ResizeObserver(() => this.#checkOverflow())
    this.resizeObserver.observe(this.textTarget)

    // Listen for window-resize to recalc
    this.windowResizeHandler = () => this.#checkOverflow()
    window.addEventListener("resize", this.windowResizeHandler)

    // Initial pass after DOM settle
    requestAnimationFrame(() => this.#checkOverflow())
  }

  disconnect() {
    if (this.resizeObserver) this.resizeObserver.disconnect()
    window.removeEventListener("resize", this.windowResizeHandler)
    if (this.animation) this.animation.cancel()
    this.#restoreOriginalContent()
  }

  /* private */

  #setupMarqueeStructure() {
    this.marqueeInitialized = false
    this.originalHTML = this.textTarget.innerHTML

    this.wrapper = document.createElement("div")
    this.wrapper.style.display = "inline-block"
    this.wrapper.style.whiteSpace = "nowrap"

    this.contentContainer = document.createElement("span")
    this.contentContainer.style.display = "inline-block"

    this.spacer = document.createElement("span")
    this.spacer.style.display = "inline-block"
    this.spacer.style.width = "4rem"
    this.spacer.innerHTML = "&nbsp;"

    this.cloneContainer = document.createElement("span")
    this.cloneContainer.style.display = "inline-block"

    // Observer to keep clone in sync with live content (e.g., countdown ticks)
    this.contentMutationObserver = new MutationObserver(() => {
      if (this.marqueeInitialized) {
        this.cloneContainer.innerHTML = this.contentContainer.innerHTML
      }
    })
  }

  #activateMarquee() {
    if (this.marqueeInitialized) return

    const text = this.textTarget
    this.originalHTML = text.innerHTML

    this.contentContainer.innerHTML = this.originalHTML
    this.cloneContainer.innerHTML = this.originalHTML

    this.wrapper.appendChild(this.contentContainer)
    this.wrapper.appendChild(this.spacer)
    this.wrapper.appendChild(this.cloneContainer)

    text.innerHTML = ""
    text.appendChild(this.wrapper)

    this.marqueeInitialized = true

    // Pause on hover
    this.wrapper.addEventListener("mouseenter", this.#pauseMarquee)
    this.wrapper.addEventListener("mouseleave", this.#resumeMarquee)

    // Start observing mutations (text changes, subtree changes)
    this.contentMutationObserver.observe(this.contentContainer, {
      characterData: true,
      subtree: true,
      childList: true,
    })
  }

  #restoreOriginalContent() {
    if (!this.marqueeInitialized) return
    // Remove hover listeners if present
    if (this.wrapper) {
      this.wrapper.removeEventListener("mouseenter", this.#pauseMarquee)
      this.wrapper.removeEventListener("mouseleave", this.#resumeMarquee)
    }
    this.textTarget.innerHTML = this.originalHTML
    this.marqueeInitialized = false

    if (this.contentMutationObserver) this.contentMutationObserver.disconnect()
  }

  // Pause/resume handlers
  #pauseMarquee = () => {
    if (this.animation) this.animation.pause()
  }

  #resumeMarquee = () => {
    if (this.animation) this.animation.play()
  }

  #updateClonedContent() {
    if (this.marqueeInitialized) {
      this.cloneContainer.innerHTML = this.contentContainer.innerHTML
    }
  }

  #checkOverflow() {
    const container = this.element
    const text = this.textTarget

    // Update cloned content if needed
    this.#updateClonedContent()

    const isOverflowing = text.scrollWidth > container.clientWidth

    if (isOverflowing) {
      if (!this.marqueeInitialized) this.#activateMarquee()

      const fullWidth = this.contentContainer.offsetWidth + this.spacer.offsetWidth

      if (this.animation) this.animation.cancel()

      const duration = Math.max(fullWidth / 50, 5) * 1000
      this.animation = this.wrapper.animate(
        [{ transform: "translateX(0)" }, { transform: `translateX(-${fullWidth}px)` }],
        { duration, iterations: Infinity, easing: "linear" }
      )
    } else {
      if (this.marqueeInitialized) {
        if (this.animation) this.animation.cancel()
        this.#restoreOriginalContent()
      }
    }
  }
}