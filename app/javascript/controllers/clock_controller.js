import { Controller } from "@hotwired/stimulus"

// Connects to data-controller="clock"
export default class extends Controller {
  static targets = ["timestamp"]
  static values = { timeZone: String }

  connect() {
    this.updateTime()
    this.timer = setInterval(() => this.updateTime(), 1000)
  }

  disconnect() {
    clearInterval(this.timer)
  }

  updateTime() {
    const now = new Date()
    const formatter = new Intl.DateTimeFormat('en-US', {
      timeZone: this.timeZoneValue,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: true
    })

    // Format the date and time to match "%Y-%m-%d %I:%M %p"
    const parts = formatter.formatToParts(now)
    const year = parts.find(p => p.type === 'year').value
    const month = parts.find(p => p.type === 'month').value
    const day = parts.find(p => p.type === 'day').value
    const hour = parts.find(p => p.type === 'hour').value
    const minute = parts.find(p => p.type === 'minute').value
    const ampm = parts.find(p => p.type === 'dayPeriod').value

    const formattedTime = `${year}-${month}-${day} ${hour}:${minute} ${ampm}`
    this.timestampTarget.textContent = formattedTime
  }
}
