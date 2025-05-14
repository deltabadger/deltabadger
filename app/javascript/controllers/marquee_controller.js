import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["text"]
  
  connect() {
    // Set initial state
    this.checkOverflow()
    
    // Check again when content changes (countdown updates, etc.)
    this.resizeObserver = new ResizeObserver(() => {
      this.checkOverflow()
    })
    
    this.resizeObserver.observe(this.textTarget)
    
    // Also check on window resize
    window.addEventListener('resize', this.checkOverflow.bind(this))
  }
  
  disconnect() {
    if (this.resizeObserver) {
      this.resizeObserver.disconnect()
    }
    window.removeEventListener('resize', this.checkOverflow.bind(this))
    
    // Clean up any animation
    if (this.animation) {
      this.animation.cancel()
    }
  }
  
  checkOverflow() {
    const container = this.element
    const text = this.textTarget
    
    // Reset any existing animation
    if (this.animation) {
      this.animation.cancel()
      text.style.animation = ''
    }
    
    // First ensure the container doesn't expand
    container.style.width = container.offsetWidth + 'px'
    
    // Check if text is wider than container
    const textWidth = text.scrollWidth
    const containerWidth = container.clientWidth - parseInt(window.getComputedStyle(container).paddingLeft) - parseInt(window.getComputedStyle(container).paddingRight)
    const isOverflowing = textWidth > containerWidth
    
    if (isOverflowing) {
      // Calculate animation duration based on text length
      const duration = Math.max(textWidth / 50, 5) // Adjust speed as needed
      
      // Set up animation
      this.animation = text.animate(
        [
          { transform: 'translateX(0)' },
          { transform: `translateX(-${textWidth - containerWidth}px)` }
        ],
        {
          duration: duration * 1000,
          iterations: Infinity,
          delay: 1000,
          easing: 'linear',
          direction: 'alternate',
          endDelay: 1000
        }
      )
    } else {
      // If not overflowing, ensure text is visible and centered
      text.style.transform = 'translateX(0)'
    }
  }
} 