import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["text"]
  
  connect() {
    // Store original text when connecting
    this.originalText = this.textTarget.innerHTML
    
    // Set initial state
    this.checkOverflow()
    
    // Check again when content changes (countdown updates, etc.)
    this.resizeObserver = new ResizeObserver(() => {
      // Update original text when content changes
      this.originalText = this.textTarget.innerHTML
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
    
    // Restore original text
    this.textTarget.innerHTML = this.originalText
  }
  
  checkOverflow() {
    const container = this.element
    const text = this.textTarget
    
    // Reset any existing animation
    if (this.animation) {
      this.animation.cancel()
      text.style.animation = ''
      text.innerHTML = this.originalText
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
      
      // Create a continuous left-moving animation
      // First clone the text and append it to itself to create a seamless loop
      text.innerHTML = this.originalText + '<span style="margin-left: 2rem;">' + this.originalText + '</span>'
      
      // Set up animation that resets position once the first text is out of view
      this.animation = text.animate(
        [
          { transform: 'translateX(0)' },
          { transform: `translateX(-${textWidth + 32}px)` } // 32px accounts for the 2rem margin
        ],
        {
          duration: duration * 1000,
          iterations: Infinity,
          delay: 1000,
          easing: 'linear'
        }
      )
      
      // When animation completes one cycle, reset position to create illusion of infinite scroll
      this.animation.onfinish = () => {
        text.style.transform = 'translateX(0)'
      }
    } else {
      // If not overflowing, ensure text is visible and reset to original
      text.style.transform = 'translateX(0)'
      text.innerHTML = this.originalText
    }
  }
} 