import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="index-filter"
// Filters index tiles based on search input matching name or description
// Prioritizes name matches over description-only matches
export default class extends Controller {
  static targets = ["input", "tile", "empty", "container"]

  filter() {
    const query = this.inputTarget.value.toLowerCase().trim()

    if (query === "") {
      this.showAll()
      return
    }

    const matches = []
    this.tileTargets.forEach(tile => {
      const name = (tile.dataset.indexName || "").toLowerCase()
      const description = (tile.dataset.indexDescription || "").toLowerCase()
      const nameMatches = name.includes(query)
      const descMatches = description.includes(query)

      if (nameMatches || descMatches) {
        // Score: name match = 2, description only = 1
        // Exact name match or starts with = 3
        let score = 0
        if (name === query || name.startsWith(query + " ") || name.startsWith(query)) {
          score = 3
        } else if (nameMatches) {
          score = 2
        } else {
          score = 1
        }
        matches.push({ tile, score })
        tile.style.display = ""
      } else {
        tile.style.display = "none"
      }
    })

    // Sort by score descending and reorder in DOM
    matches.sort((a, b) => b.score - a.score)
    const container = this.tileTargets[0]?.parentElement
    if (container) {
      matches.forEach(({ tile }) => container.appendChild(tile))
    }

    this.toggleEmpty(matches.length === 0)
  }

  showAll() {
    // Restore original order by sorting by data-index-order
    const tiles = [...this.tileTargets]
    tiles.sort((a, b) => {
      const orderA = parseInt(a.dataset.indexOrder || "0", 10)
      const orderB = parseInt(b.dataset.indexOrder || "0", 10)
      return orderA - orderB
    })

    const container = tiles[0]?.parentElement
    if (container) {
      tiles.forEach(tile => {
        tile.style.display = ""
        container.appendChild(tile)
      })
    }
    this.toggleEmpty(false)
  }

  toggleEmpty(show) {
    if (this.hasEmptyTarget) {
      this.emptyTarget.style.display = show ? "" : "none"
    }
  }
}
