import React, { useState, useEffect, useMemo } from 'react';
import I18n from 'i18n-js';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';

const calculateDelay = (nextTimestamp, nowTimestamp) => {
  return nextTimestamp - nowTimestamp;
};

const calculateInterval = (delay) => {
  if (delay >= 0) {
    return 1000;
  } else {
    return Math.abs(delay) * 1000;
  }
};

export const Timer = ({ bot, callback }) => {
  const [i, setI] = useState(0);
  const { settings, status, nextTransactionTimestamp, nowTimestamp } = bot || { settings: {}, stats: {}, transactions: [], logs: [] };
  
  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, nowTimestamp));
  const timeout = delay < 0;

  const infotext = useMemo(() => {
    return formatDuration(moment.duration(delay, 'seconds'));
  }, [delay]);

  useEffect(() => { 
    setDelay(calculateDelay(nextTransactionTimestamp, nowTimestamp));
    setI(0);
  }, [bot.nextTransactionTimestamp]);

  useInterval(() => {
    if (timeout && i === 0) {
      if (bot) {
        setI(i + 1);
        callback(bot);
      }
    }
    setDelay(prev => prev - 1);
  }, calculateInterval(delay));

  if (timeout) { 
    return <Spinner />; 
  }

  const countdown = formatDuration(moment.duration(delay, 'seconds'));
  const translation_key = settings.type === 'buy' ? 'bots.next_buy' : 'bots.next_sell';

  return (
    <span className="bot-counting" data-testid="bot-timer">
      {infotext}
    </span>
  );
};

export const FetchFromExchangeTimer = ({ bot, callback }) => {
  const [i, setI] = useState(0);
  const { status, nextResultFetchingTimestamp, nowTimestamp } = bot || { settings: {}, stats: {}, transactions: [], logs: [] };

  const [delay, setDelay] = useState(calculateDelay(nextResultFetchingTimestamp, nowTimestamp, status));
  const timeout = delay < 0;

  useEffect(() => { 
    setDelay(calculateDelay(nextResultFetchingTimestamp, nowTimestamp, status));
  }, [bot.nextResultFetchingTimestamp]);

  useInterval(() => {
    if (timeout && i === 0) {
      if (bot) {
        setI(i + 1);
        callback(bot);
      }
    }
    setDelay(delay - 1);
  }, 1000);

  if (timeout) { return <Spinner />; }

  return (
    <div className="db-bot__infotext__right">
      {I18n.t('bots.buttons.pending.info_html')}
    </div>
  );
};
