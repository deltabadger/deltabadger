import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';


const calculateDelay = (nextTransactionTimestamp, now) => {
  const date = moment.unix(nextTransactionTimestamp)

  const a = moment.duration(date.diff(now))
  return a
}

export const Timer = ({bot, callback, isPending}) => {
  let i = 0;
  const { settings, status, nextTransactionTimestamp, nowTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const working = status == 'working'

  const [delay, setDelay] = useState(calculateDelay())
  const [now, setNow] = useState(moment.unix(nowTimestamp))
  const [pending, setPending] = useState(false)
  const timeout = delay.seconds() < 0

  useInterval(() => {
    const calculatedDelay = calculateDelay(nextTransactionTimestamp, now)

    if(timeout && !isPending && i == 0) {
      if (bot) {
        i = i + 1;
        callback(bot)
      }
    }
    setNow(now.add(1, 'seconds'))
    setDelay(calculatedDelay)
  }, 1000);

  if (timeout) { return <Spinner /> }

  return (
    <div className="db-bot__infotext__right">
      Next { settings.type } in { formatDuration(delay) }
    </div>
  )
}
