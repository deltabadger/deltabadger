import React, { useState, useEffect, useMemo, useCallback } from 'react';
import I18n from 'i18n-js';
import moment from 'moment';
import { formatDuration } from '../utils/time';
import { Spinner } from './Spinner';

const calculateDelay = (nextTimestamp, nowTimestamp) => {
  return nextTimestamp - nowTimestamp;
};

export const Timer = ({ bot, callback }) => {
  const [i, setI] = useState(0);
  const { settings, status, nextTransactionTimestamp, nowTimestamp } = bot || { settings: {}, stats: {}, transactions: [], logs: [] };
  
  const [delay, setDelay] = useState(calculateDelay(nextTransactionTimestamp, nowTimestamp));
  const timeout = delay < 0;

  const infotext = useMemo(() => {
    return formatDuration(moment.duration(delay, 'seconds'));
  }, [delay]);

  // Reset state when transaction timestamp changes
  useEffect(() => { 
    setDelay(calculateDelay(nextTransactionTimestamp, nowTimestamp));
    setI(0);
  }, [nextTransactionTimestamp, nowTimestamp]);

  // Handle interval and cleanup
  useEffect(() => {
    let intervalId;
    const interval = timeout ? Math.abs(delay) * 1000 : 1000;

    intervalId = setInterval(() => {
      if (timeout && i === 0) {
        if (bot) {
          setI(prev => prev + 1);
          callback(bot);
        }
      }
      setDelay(prev => prev - 1);
    }, interval);

    // Cleanup function
    return () => {
      if (intervalId) {
        clearInterval(intervalId);
      }
    };
  }, [timeout, i, bot, delay, callback]);

  if (timeout) { 
    return <Spinner />; 
  }

  return (
    <span className="bot-counting" data-testid="bot-timer">
      {infotext}
    </span>
  );
};

export const FetchFromExchangeTimer = ({ bot, callback }) => {
  const [i, setI] = useState(0);
  const { status, nextResultFetchingTimestamp, nowTimestamp } = bot || { settings: {}, stats: {}, transactions: [], logs: [] };

  const [delay, setDelay] = useState(calculateDelay(nextResultFetchingTimestamp, nowTimestamp));
  const timeout = delay < 0;

  // Reset state when fetching timestamp changes
  useEffect(() => { 
    setDelay(calculateDelay(nextResultFetchingTimestamp, nowTimestamp));
  }, [nextResultFetchingTimestamp, nowTimestamp]);

  // Handle interval and cleanup
  useEffect(() => {
    let intervalId;

    intervalId = setInterval(() => {
      if (timeout && i === 0) {
        if (bot) {
          setI(prev => prev + 1);
          callback(bot);
        }
      }
      setDelay(prev => prev - 1);
    }, 1000);

    // Cleanup function
    return () => {
      if (intervalId) {
        clearInterval(intervalId);
      }
    };
  }, [timeout, i, bot, callback]);

  if (timeout) { 
    return <Spinner />; 
  }

  return (
    <div className="db-bot__infotext__right">
      {I18n.t('bots.buttons.pending.info_html')}
    </div>
  );
};
