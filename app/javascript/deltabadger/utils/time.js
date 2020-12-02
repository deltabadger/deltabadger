const buildFormattedDuration = (formattedDuration, el) => {
  if (el.number && el.number >= 1) {
    // return formattedDuration + " " + `${String(el.number).padStart(2, '0')}${el.name}`
    return formattedDuration + " " + `${String(el.number)}${el.name}`
  } else {
    return formattedDuration
  }
}

export const formatDuration = (duration) => {
  if (!duration) { return false }

  const months = duration.months()
  const days = duration.days()
  const hours = duration.hours()
  const minutes = duration.minutes()
  const seconds = duration.seconds()

  const data = [
    { name: 'm', number: months },
    { name: 'd', number: days },
    { name: 'h', number: hours },
    { name: 'm', number: minutes },
    { name: 's', number: seconds }
  ]

  return data.reduce(buildFormattedDuration, "")
}

export const formatDurationRestart = (duration) => {
  if (!duration) { return false }

  const months = duration.months()
  const days = duration.days()
  const hours = duration.hours()
  const minutes = duration.minutes() + (duration.seconds() > 0)

  const data = [
    { name: 'm', number: months },
    { name: 'd', number: days },
    { name: 'h', number: hours },
    { name: 'm', number: minutes },
  ]

  return data.reduce(buildFormattedDuration, "")
}
