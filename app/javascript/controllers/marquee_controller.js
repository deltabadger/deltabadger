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

    // Set up wrapper and clones just once
    this.#setupMarqueeStructure()

    // Track when content changes (countdown updates, etc.)
    this.resizeObserver = new ResizeObserver(() => {
      this.#checkOverflow()
    })

    this.resizeObserver.observe(this.textTarget)

    // Handle window resize
    this.windowResizeHandler = this.#handleWindowResize.bind(this)
    window.addEventListener('resize', this.windowResizeHandler)

    // Initial overflow check (after a short delay to ensure DOM is ready)
    setTimeout(() => this.#checkOverflow(), 50)
  }

  disconnect() {
    if (this.resizeObserver) {
      this.resizeObserver.disconnect()
    }
    window.removeEventListener('resize', this.windowResizeHandler)

    // Clean up any animation
    if (this.animation) {
      this.animation.cancel()
    }

    // Restore original structure
    this.#restoreOriginalContent()

    // Remove fixed width
    this.element.style.width = ''
  }

  #handleWindowResize() {
    // Remove fixed width on window resize
    this.element.style.width = ''
    this.initialWidthSet = false

    // Recalculate
    this.#checkOverflow()
  }

  #setupMarqueeStructure() {
    // Flag to track if we've set up our marquee structure
    this.marqueeInitialized = false

    // Store original content (but don't change DOM yet)
    this.originalHTML = this.textTarget.innerHTML

    // Create wrapper for animation
    this.wrapper = document.createElement('div')
    this.wrapper.style.display = 'inline-block'
    this.wrapper.style.whiteSpace = 'nowrap'

    // Create container for original content
    this.contentContainer = document.createElement('span')
    this.contentContainer.style.display = 'inline-block'

    // Create spacer
    this.spacer = document.createElement('span')
    this.spacer.style.display = 'inline-block'
    this.spacer.style.width = '4rem'
    this.spacer.innerHTML = '&nbsp;'

    // Create container for cloned content
    this.cloneContainer = document.createElement('span')
    this.cloneContainer.style.display = 'inline-block'
  }

  #activateMarquee() {
    if (this.marqueeInitialized) return;

    const text = this.textTarget

    // Save current content
    this.originalHTML = text.innerHTML

    // Set up content containers
    this.contentContainer.innerHTML = this.originalHTML
    this.cloneContainer.innerHTML = this.originalHTML

    // Build DOM structure
    this.wrapper.appendChild(this.contentContainer)
    this.wrapper.appendChild(this.spacer)
    this.wrapper.appendChild(this.cloneContainer)

    // Replace content with our wrapper
    text.innerHTML = ''
    text.appendChild(this.wrapper)

    this.marqueeInitialized = true
  }

  #restoreOriginalContent() {
    if (!this.marqueeInitialized || !this.originalHTML) return;

    this.textTarget.innerHTML = this.originalHTML
    this.marqueeInitialized = false
  }

  #updateClonedContent() {
    // Update clone to match current content (which may have changed due to countdown)
    if (this.marqueeInitialized) {
      // Get current content from the first container
      const currentHTML = this.contentContainer.innerHTML
      // Update the cloned content
      this.cloneContainer.innerHTML = currentHTML
    }
  }

  #checkOverflow() {
    const container = this.element
    const text = this.textTarget

    // Get current container width (responsive to window size)
    const currentContainerWidth = container.offsetWidth || container.parentElement.offsetWidth * 0.9

    // Important: Ensure container width is fixed to prevent expansion from content
    // Only set it if not already set or if window has resized
    if (!this.initialWidthSet) {
      // Force the container to maintain its current width
      container.style.width = `${currentContainerWidth}px`
      this.initialWidthSet = true
    }

    // If marquee is active, update the cloned content
    this.#updateClonedContent()

    // Get current content width
    let contentWidth = text.scrollWidth;
    let visibleWidth = container.clientWidth;

    // Check if text would overflow
    const isOverflowing = contentWidth > visibleWidth;

    if (isOverflowing) {
      // Activate marquee if not already active
      if (!this.marqueeInitialized) {
        this.#activateMarquee();

        // Recalculate sizes after DOM changes
        contentWidth = this.contentContainer.offsetWidth;
      }

      // Cancel any existing animation
      if (this.animation) {
        this.animation.cancel();
      }

      // Calculate the full width to animate
      const fullContentWidth = this.contentContainer.offsetWidth + this.spacer.offsetWidth;

      // Calculate animation duration based on content width
      const duration = Math.max(fullContentWidth / 50, 5); // Adjust speed as needed

      // Create a smooth animation
      this.animation = this.wrapper.animate(
        [
          { transform: 'translateX(0)' },
          { transform: `translateX(-${fullContentWidth}px)` }
        ],
        {
          duration: duration * 1000,
          iterations: Infinity,
          easing: 'linear'
        }
      );
    } else {
      // If not overflowing and marquee is active, deactivate it
      if (this.marqueeInitialized) {
        // Cancel any animation
        if (this.animation) {
          this.animation.cancel();
        }

        // Restore original content
        this.#restoreOriginalContent();
      }
    }
  }
}