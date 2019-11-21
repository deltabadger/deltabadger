import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';

export const Timer = ({bot, callback}) => {
  const { settings, status, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const working = status == 'working'

  const calculateDelay = () => {
    const now = new moment()
    const date = nextTransactionTimestamp && new moment.unix(nextTransactionTimestamp)

    return nextTransactionTimestamp && moment.duration(date.diff(now))
  }

  const [delay, setDelay] = useState(calculateDelay())
  const [pending, setPending] = useState(false)
  const timeout = delay.seconds() < 0

  useInterval(() => {
    const calculatedDelay = calculateDelay()
    if(timeout && !pending) {
      setPending(true)
      if (bot) {
        console.log('odpalam callback')
        callback(bot)
      }
    }
    setDelay(calculatedDelay)
  }, 1000);

  if (timeout) { return <Spinner /> }

  return (
    <div className="db-bot__infotext__right">
      Next { settings.type } in { formatDuration(delay) }
    </div>
  )
}
