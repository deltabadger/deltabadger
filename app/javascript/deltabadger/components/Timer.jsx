import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';

const calculateDelay = (nextTransactionTimestamp, nowTimestamp) => {
  return nextTransactionTimestamp - nowTimestamp
}

export const Timer = ({bot, callback}) => {
  let i = 0;
  const { settings, nextTransactionTimestamp, nowTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, nowTimestamp))
  const timeout = delay < 0

  useEffect(() => { setDelay(calculateDelay(nextTransactionTimestamp, nowTimestamp))}, [bot.nextTransactionTimestamp])
  useInterval(() => {
    if(timeout && i == 0) {
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
