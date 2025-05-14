import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["text"]
  
  connect() {
    // Cache container width on connect
    this.containerWidth = this.element.offsetWidth
    
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
    
    // Restore original state
    if (this.originalHTML) {
      this.textTarget.innerHTML = this.originalHTML
    }
    
    // Remove fixed width
    this.element.style.width = ''
  }
  
  checkOverflow() {
    const container = this.element
    const text = this.textTarget
    
    // Reset any existing animation
    if (this.animation) {
      this.animation.cancel()
      if (this.originalHTML) {
        text.innerHTML = this.originalHTML
      }
    }
    
    // Store original content if not already stored
    if (!this.originalHTML) {
      this.originalHTML = text.innerHTML
    }
    
    // Important: Ensure container width is fixed based on initial width
    // Only set it once to avoid growth
    if (!this.initialWidthSet) {
      // Force the container to maintain its width
      container.style.width = `${this.containerWidth}px`
      this.initialWidthSet = true
    }
    
    // Check if text is wider than container
    const textWidth = text.scrollWidth
    const containerWidth = container.clientWidth
    const isOverflowing = textWidth > containerWidth
    
    if (isOverflowing) {
      // Create a continuous left-moving animation
      // We'll use a wrapper to handle nested content properly
      const wrapper = document.createElement('div')
      wrapper.style.display = 'inline-block'
      wrapper.style.whiteSpace = 'nowrap'
      
      // Clone the original content for a seamless loop
      // For a smooth loop, we need multiple copies with proper spacing
      const originalClone = document.createElement('span')
      originalClone.innerHTML = this.originalHTML
      originalClone.style.display = 'inline-block'
      
      const spacer = document.createElement('span')
      spacer.style.display = 'inline-block'
      spacer.style.width = '4rem' // Double the padding on the text element
      spacer.innerHTML = '&nbsp;'
      
      const secondClone = document.createElement('span')
      secondClone.innerHTML = this.originalHTML
      secondClone.style.display = 'inline-block'
      
      // Add elements to create a smooth loop
      wrapper.appendChild(originalClone)
      wrapper.appendChild(spacer)
      wrapper.appendChild(secondClone)
      
      // Replace the content with our wrapper
      text.innerHTML = ''
      text.appendChild(wrapper)
      
      // Calculate the full width of content with spacer
      const fullContentWidth = originalClone.offsetWidth + spacer.offsetWidth
      
      // Calculate animation duration based on content width
      const duration = Math.max(fullContentWidth / 50, 5) // Adjust speed as needed
      
      // Create a smooth infinite loop animation
      this.animation = wrapper.animate(
        [
          { transform: 'translateX(0)' },
          { transform: `translateX(-${fullContentWidth}px)` }
        ],
        {
          duration: duration * 1000,
          iterations: Infinity,
          easing: 'linear'
        }
      )
      
      // Create a seamless loop by using the CSS Animation API's onfinish callback
      this.animation.onfinish = () => {
        wrapper.style.transform = 'translateX(0)'
        this.animation.play()
      }
    } else {
      // If not overflowing, ensure text is visible and reset to original
      text.innerHTML = this.originalHTML
    }
  }
} 