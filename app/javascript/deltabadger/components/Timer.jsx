import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';


const calculateDelay = (nextTransactionTimestamp, now) => {
  const date = moment.unix(nextTransactionTimestamp).utc()
  const a = moment.duration(date.diff(now))
  return a
}

export const Timer = ({bot, callback, isPending}) => {
  let i = 0;
  const { settings, status, nextTransactionTimestamp, nowTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const working = status == 'working'

  console.log('normal', moment.unix(nowTimestamp))
  console.log('utc', moment.unix(nowTimestamp).utc())
  console.log('nextTransactionTimestamp', nextTransactionTimestamp)

  const [now, setNow] = useState(moment.unix(nowTimestamp).utc())
  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, now))
  const [pending, setPending] = useState(false)
  const timeout = delay.seconds() < 0

  useInterval(() => {

    if(timeout && !isPending && i == 0) {
      if (bot) {
        i = i + 1;
        callback(bot)
      }
    }
    setNow(now.add(1, 'seconds'))
    setDelay(
      calculateDelay(nextTransactionTimestamp, now)
    )
  }, 1000);

  if (timeout) { return <Spinner /> }

  return (
    <div className="db-bot__infotext__right">
      Next { settings.type } in { formatDuration(delay) }
    </div>
  )
}
