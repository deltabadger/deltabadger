import { Controller } from "@hotwired/stimulus"

// TODO: Use chartjs lib instead. https://www.chartjs.org/docs/latest/samples/other-charts/pie.html

export default class extends Controller {
  static targets = ["svg", "legend"]
  static values = { data: String }

  connect() {
    this.width = 300
    this.height = 300
    this.radius = Math.min(this.width, this.height) / 2 - 10
    this.centerX = this.width / 2
    this.centerY = this.height / 2
    
    if (this.dataValue) {
      this.render(this.dataValue)
    }
  }

  parseCSV(csvData) {
    const lines = csvData.trim().split('\n')
    const data = []
    let currentColumn = 0
    
    lines.forEach(line => {
      if (line.trim() === 'COLUMN_BREAK') {
        currentColumn++
        return
      }
      
      const [color, symbol, name, percentage] = line.split(',')
      data.push({
        color: color.trim(),
        symbol: symbol.trim(),
        name: name.trim(),
        percentage: parseFloat(percentage.trim()),
        column: currentColumn
      })
    })
    
    return data
  }

  createSlicePath(startAngle, endAngle, radius = this.radius) {
    const startX = this.centerX + radius * Math.cos(startAngle)
    const startY = this.centerY + radius * Math.sin(startAngle)
    const endX = this.centerX + radius * Math.cos(endAngle)
    const endY = this.centerY + radius * Math.sin(endAngle)
    
    const largeArcFlag = endAngle - startAngle <= Math.PI ? 0 : 1
    
    return `M ${this.centerX} ${this.centerY} L ${startX} ${startY} A ${radius} ${radius} 0 ${largeArcFlag} 1 ${endX} ${endY} Z`
  }

  render(csvData) {
    const data = this.parseCSV(csvData)
    
    // Clear existing content
    this.svgTarget.innerHTML = ''
    this.legendTarget.innerHTML = ''
    
    // Determine number of columns needed
    const maxColumn = Math.max(...data.map(item => item.column))
    const hasMultipleColumns = maxColumn > 0
    
    // Create column containers if needed
    if (hasMultipleColumns) {
      this.legendTarget.classList.add('legend-two-columns')
      for (let i = 0; i <= maxColumn; i++) {
        const column = document.createElement('div')
        column.classList.add('legend-column')
        column.dataset.column = i
        this.legendTarget.appendChild(column)
      }
    } else {
      this.legendTarget.classList.remove('legend-two-columns')
    }
    
    let currentAngle = -Math.PI / 2 // Start from top
    
    data.forEach((item, index) => {
      const sliceAngle = (item.percentage / 100) * 2 * Math.PI
      const endAngle = currentAngle + sliceAngle
      
      // Create slice
      const slice = document.createElementNS('http://www.w3.org/2000/svg', 'path')
      slice.setAttribute('d', this.createSlicePath(currentAngle, endAngle))
      slice.setAttribute('fill', item.color)
      slice.setAttribute('stroke', 'none')
      slice.classList.add('pie-slice')
      slice.dataset.index = index
      slice.dataset.startAngle = currentAngle
      slice.dataset.endAngle = endAngle
      
      this.svgTarget.appendChild(slice)
      
      // Create legend item
      const legendItem = document.createElement('div')
      legendItem.classList.add('legend-item')
      legendItem.dataset.index = index
      legendItem.innerHTML = `
        <div class="legend-color" style="background-color: ${item.color}"></div>
        <span class="legend-label">${item.name}</span>
        <span class="legend-value">${item.percentage}%</span>
      `
      
      // Add to appropriate column
      if (hasMultipleColumns) {
        const columnElement = this.legendTarget.querySelector(`[data-column="${item.column}"]`)
        columnElement.appendChild(legendItem)
      } else {
        this.legendTarget.appendChild(legendItem)
      }
      
      currentAngle = endAngle
    })

    // Add event listeners after all elements are created
    this.addHoverEffects()
  }

  addHoverEffects() {
    const slices = this.svgTarget.querySelectorAll('.pie-slice')
    const legendItems = this.legendTarget.querySelectorAll('.legend-item')

    // Pie slice hover effects
    slices.forEach((slice, index) => {
      slice.addEventListener('mouseenter', () => {
        slice.style.opacity = '1'
        const startAngle = parseFloat(slice.dataset.startAngle)
        const endAngle = parseFloat(slice.dataset.endAngle)
        slice.setAttribute('d', this.createSlicePath(startAngle, endAngle, this.radius + 5))
        const legendItem = Array.from(legendItems).find(item => item.dataset.index == index)
        if (legendItem) legendItem.classList.add('highlighted')
      })
      
      slice.addEventListener('mouseleave', () => {
        slice.style.opacity = '0.75'
        const startAngle = parseFloat(slice.dataset.startAngle)
        const endAngle = parseFloat(slice.dataset.endAngle)
        slice.setAttribute('d', this.createSlicePath(startAngle, endAngle, this.radius))
        const legendItem = Array.from(legendItems).find(item => item.dataset.index == index)
        if (legendItem) legendItem.classList.remove('highlighted')
      })
    })

    // Legend item hover effects
    legendItems.forEach((legendItem) => {
      const index = parseInt(legendItem.dataset.index)
      
      legendItem.addEventListener('mouseenter', () => {
        const slice = slices[index]
        slice.style.opacity = '1'
        const startAngle = parseFloat(slice.dataset.startAngle)
        const endAngle = parseFloat(slice.dataset.endAngle)
        slice.setAttribute('d', this.createSlicePath(startAngle, endAngle, this.radius + 10))
        legendItem.classList.add('highlighted')
      })
      
      legendItem.addEventListener('mouseleave', () => {
        const slice = slices[index]
        slice.style.opacity = '0.75'
        const startAngle = parseFloat(slice.dataset.startAngle)
        const endAngle = parseFloat(slice.dataset.endAngle)
        slice.setAttribute('d', this.createSlicePath(startAngle, endAngle, this.radius))
        legendItem.classList.remove('highlighted')
      })
    })
  }
} 
