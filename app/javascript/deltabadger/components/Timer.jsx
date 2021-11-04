import React, { useState, useEffect } from 'react';
import I18n from 'i18n-js'
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';

const calculateDelay = (nextTimestamp, nowTimestamp) => {
  return nextTimestamp - nowTimestamp
}

const calculateInterval = (delay) => {
  if (delay >= 0) {
    return 1000
  } else {
    return Math.abs(delay) * 1000
  }
}

export const Timer = ({bot, callback}) => {
  let i = 0;
  const { settings, status, nextTransactionTimestamp, nowTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

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
  }, calculateInterval(delay));

  if (timeout) { return <Spinner /> }

  const countdown = formatDuration(moment.duration(delay, 'seconds'))
  const translation_key = settings.type === 'buy' ? 'bots.next_buy' : 'bots.next_sell'

  return (
    <div className="db-bot__infotext__right">
      <span className="d-none d-sm-inline">{bot.bot_type === 'free' ? I18n.t(translation_key, { countdown }) : `Next withdrawal in ${countdown}`}</span>
    </div>
  )
}

export const FetchFromExchangeTimer = ({bot, callback}) => {
  let i = 0;
  const { status, nextResultFetchingTimestamp, nowTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}

  const [delay, setDelay] = useState(calculateDelay(nextResultFetchingTimestamp, nowTimestamp, status))
  const timeout = delay < 0

  useEffect(() => { setDelay(calculateDelay(nextResultFetchingTimestamp, nowTimestamp, status))}, [bot.nextResultFetchingTimestamp])
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
      {I18n.t('bots.buttons.pending.info_html')}
    </div>
  )
}
