import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["text"]
  
  connect() {
    // Cache container width on connect
    this.containerWidth = this.element.offsetWidth
    
    // Flag to track if we've set up our marquee structure
    this.marqueeInitialized = false
    
    // Set up wrapper and clones just once
    this.setupMarqueeStructure()
    
    // Check for overflow
    this.checkOverflow()
    
    // Track content changes (countdown updates, etc.)
    this.resizeObserver = new ResizeObserver(() => {
      // Only need to check overflow, DOM structure stays intact
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
    
    // Restore original structure
    this.restoreOriginalContent()
  }
  
  setupMarqueeStructure() {
    // Store original content (but don't change DOM yet)
    this.originalHTML = this.textTarget.innerHTML
    
    // Create marquee DOM structure but keep it inactive
    const text = this.textTarget
    
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
  
  activateMarquee() {
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
  
  restoreOriginalContent() {
    if (!this.marqueeInitialized || !this.originalHTML) return;
    
    this.textTarget.innerHTML = this.originalHTML
    this.marqueeInitialized = false
  }
  
  updateClonedContent() {
    // Update clone to match current content (which may have changed due to countdown)
    if (this.marqueeInitialized) {
      // Get current content from the first container
      const currentHTML = this.contentContainer.innerHTML
      // Update the cloned content
      this.cloneContainer.innerHTML = currentHTML
    }
  }
  
  checkOverflow() {
    const container = this.element
    const text = this.textTarget
    
    // Important: Ensure container width is fixed based on initial width
    // Only set it once to avoid growth
    if (!this.initialWidthSet) {
      // Force the container to maintain its width
      container.style.width = `${this.containerWidth}px`
      this.initialWidthSet = true
    }
    
    // If marquee is active, update the cloned content
    this.updateClonedContent()
    
    // Get current content width
    let contentWidth = text.scrollWidth;
    let containerWidth = container.clientWidth;
    
    // Check if text would overflow
    // Use a temporary clone to measure the width of the original content 
    // without affecting the current DOM
    const isOverflowing = contentWidth > containerWidth;
    
    if (isOverflowing) {
      // Activate marquee if not already active
      if (!this.marqueeInitialized) {
        this.activateMarquee();
        
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
        this.restoreOriginalContent();
      }
    }
  }
} 