import React, { useState, useEffect } from 'react';
import I18n from 'i18n-js'
import { useInterval } from '../utils/interval';
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

export const PercentageProgress = ({bot, callback}) => {
  let i = 0;
  const { nextTransactionTimestamp, nowTimestamp, progressPercentage } = bot || {stats: {}, transactions: [], logs: []}

  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, nowTimestamp))
  const timeout = delay < 0

  useEffect(() => { setDelay(calculateDelay(nextTransactionTimestamp, nowTimestamp))}, [bot.nextTransactionTimestamp])
  useInterval(() => {
    if(timeout && i === 0) {
      if (bot) {
        i = i + 1;
        callback(bot)
      }
    }
    setDelay(delay - 1)
  }, calculateInterval(delay));

  if (timeout) { return <Spinner /> }

  const percentage = progressPercentage >= 1 ? Math.floor(progressPercentage) : parseFloat(progressPercentage).toFixed(2);
  return (
    <div className="db-bot__infotext__right">
      <span className="d-none d-sm-inline">{I18n.t('bots.withdrawal_percentage', {percentage: percentage})}</span>
    </div>
  )
}
