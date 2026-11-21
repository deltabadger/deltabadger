// The flash stack's geometry (controllers/flash_stack_controller.js), in px. `heights` are the toasts' own heights, newest
// first. Collapsed, the ones behind take the front one's height and peek out 10px apart above it, three at most; open,
// each sits 8px above the next at its own height.
export function stackLayout(heights, open) {
  const front = heights[0] || 0
  let y = 0
  const items = heights.map((h, i) => {
    const item = { height: open ? h : front, y: open ? -y : -i * 10, scale: open ? 1 : 1 - i * 0.05, opacity: !open && i > 2 ? 0 : 1, back: !open && i > 0 }
    y += h + 8
    return item
  })
  return { items, height: heights.length === 0 ? 0 : open ? y - 8 : front + 20 }
}
