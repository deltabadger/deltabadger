import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="tradingview"
export default class extends Controller {
  connect() {
    // Use requestAnimationFrame to ensure the element is in the DOM and visible
    requestAnimationFrame(() => {
      this.loadWidget()
    })
  }

  loadWidget() {
    // Check if the widget container exists
    const widgetContainer = this.element.querySelector('.tradingview-widget-container__widget')
    if (!widgetContainer) return

    // Find the script tag with the widget configuration
    const scriptTag = this.element.querySelector('script[src*="tradingview.com"]')
    if (!scriptTag) return

    // Extract the configuration from the script tag
    const scriptContent = scriptTag.textContent || scriptTag.innerText
    let config
    try {
      config = JSON.parse(scriptContent.trim())
    } catch (e) {
      console.error('Failed to parse TradingView widget configuration:', e)
      return
    }

    // Clear any existing content
    widgetContainer.innerHTML = ''

    // Load TradingView script if not already loaded
    if (!window.TradingView) {
      this.loadTradingViewScript(() => {
        this.createWidget(config, widgetContainer)
      })
    } else {
      this.createWidget(config, widgetContainer)
    }
  }

  createWidget(config, container) {
    // Ensure container has an ID
    if (!container.id) {
      container.id = 'tradingview_' + Date.now() + '_' + Math.random().toString(36).substr(2, 9)
    }

    // Create widget using TradingView's widget constructor
    try {
      new window.TradingView.widget({
        ...config,
        container_id: container.id
      })
    } catch (e) {
      console.error('Failed to create TradingView widget:', e)
      // Fallback: try to reinject the script
      this.fallbackScriptInjection(config, container)
    }
  }

  fallbackScriptInjection(config, container) {
    // Create a new script element with the configuration
    const script = document.createElement('script')
    script.type = 'text/javascript'
    script.src = 'https://s3.tradingview.com/external-embedding/embed-widget-symbol-overview.js'
    script.async = true
    script.textContent = JSON.stringify(config)
    
    // Append to container
    container.appendChild(script)
  }

  loadTradingViewScript(callback) {
    const script = document.createElement('script')
    script.src = 'https://s3.tradingview.com/external-embedding/embed-widget-symbol-overview.js'
    script.async = true
    script.onload = callback
    script.onerror = () => {
      console.error('Failed to load TradingView script')
    }
    document.head.appendChild(script)
  }

  // Method to reload widget (can be called manually if needed)
  reload() {
    this.loadWidget()
  }
} 