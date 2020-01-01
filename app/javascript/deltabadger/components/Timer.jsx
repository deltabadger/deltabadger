import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';


const calculateDelay = (nextTransactionTimestamp, nowTimestamp) => {
  // const date = moment.unix(nextTransactionTimestamp).utc()
  // const a = moment.duration(date.diff(now))

  const diff = nextTransactionTimestamp - nowTimestamp
  // const a = moment.duration(diff, 'seconds')
  console.log('nowTimestamp', nowTimestamp)
  console.log('nextTransactionTimestamp', nextTransactionTimestamp)
  // console.log('duration', a)
  return diff
}

export const Timer = ({bot, callback, isPending}) => {
  let i = 0;
  const { settings, status, nextTransactionTimestamp, nowTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const working = status == 'working'

  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, nowTimestamp))
  const [pending, setPending] = useState(false)
  const timeout = delay < 0

  useEffect(() => { setDelay(calculateDelay(nextTransactionTimestamp, nowTimestamp))}, [bot.nextTransactionTimestamp])
  useInterval(() => {
    if(timeout && !isPending && i == 0) {
      if (bot) {
        i = i + 1;
        callback(bot)
      }
    }
    setDelay(delay - 1)
  }, 1000);

  if (timeout) { return <Spinner /> }

  return (
    <div className="db-bot__infotext__right">
      Next { settings.type } in { formatDuration(moment.duration(delay, 'seconds')) }
    </div>
  )
}
