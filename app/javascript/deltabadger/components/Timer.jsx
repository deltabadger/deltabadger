import React, { useState, useEffect } from 'react';
import I18n from 'i18n-js'
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

  const countdown = formatDuration(moment.duration(delay, 'seconds'))
  const translation_key = settings.type === 'buy' ? 'bots.next_buy' : 'bots.next_sell'

  return (
    <div className="db-bot__infotext__right">
      {I18n.t(translation_key, { countdown })}
    </div>
  )
}
