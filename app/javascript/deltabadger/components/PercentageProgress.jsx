import React, { useState, useEffect } from 'react';
import I18n from 'i18n-js'
import { Spinner } from './Spinner';

const calculateDelay = (nextTimestamp, nowTimestamp) => {
  return nextTimestamp - nowTimestamp
}

export const PercentageProgress = ({bot, callback}) => {
  const [count, setCount] = useState(0);
  const { nextTransactionTimestamp, nowTimestamp, progressPercentage } = bot || {};
  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, nowTimestamp));
  const timeout = delay < 0;

  useEffect(() => { 
    setDelay(calculateDelay(nextTransactionTimestamp, nowTimestamp));
  }, [nextTransactionTimestamp, nowTimestamp]);

  useEffect(() => {
    let intervalId;
    const interval = timeout ? Math.abs(delay) * 1000 : 1000;

    intervalId = setInterval(() => {
      if (timeout && count === 0) {
        if (bot) {
          setCount(prev => prev + 1);
          callback(bot);
        }
      }
      setDelay(prev => prev - 1);
    }, interval);

    return () => {
      if (intervalId) {
        clearInterval(intervalId);
      }
    };
  }, [timeout, count, bot, delay, callback]);

  if (timeout) { return <Spinner /> }

  const percentage = progressPercentage >= 1 ? Math.floor(progressPercentage) : parseFloat(progressPercentage).toFixed(2);
  return (
    <div className="db-bot__infotext__right">
      <span className="d-none d-sm-inline">{I18n.t('bot.withdrawal_percentage', {percentage: percentage})}</span>
    </div>
  )
}
