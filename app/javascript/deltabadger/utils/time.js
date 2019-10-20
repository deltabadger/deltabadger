export const formatDuration = (duration) => {
  if (!duration) { return false }

  const data = [
    { name: 'm', number: duration.months() },
    { name: 'w', number: duration.weeks() },
    { name: 'd', number: duration.days() },
    { name: 'h', number: duration.hours() },
    { name: 'm', number: duration.minutes() },
    { name: 's', number: duration.seconds() }
  ]

  const buildFormattedDuration = (formattedDuration, el) => {
    if (el.number > 1) {
      return formattedDuration+ " " + `${String(el.number).padStart(2, '0')}${el.name}`
    } else {
      formattedDuration
    }
  }

  debugger
  return data.reduce(buildFormattedDuration, "")
}
