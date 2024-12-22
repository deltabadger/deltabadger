import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';

const ProgressBarLine = React.memo(({colorClass, progress}) => (
  <div className="progress progress--thin progress--bot-setup">
    <div 
      className={`progress-bar bg-${colorClass}`} 
      role="progressbar" 
      style={{width: `${progress}%`}} 
      aria-valuenow={Math.round(progress)} 
      aria-valuemin="0" 
      aria-valuemax="100" 
    />
  </div>
));

const intervalDisabled = (bot, settings) => {
  return bot?.bot_type === 'withdrawal' && !settings?.interval_enabled;
};

export const ProgressBar = React.memo(({bot}) => {
  const { settings, status, nextTransactionTimestamp, transactions, skippedTransactions} = bot || {};
  const colorClass = settings?.type === 'buy' ? 'success' : 'danger';
  const working = status === 'working';
  const isDisabled = !working || intervalDisabled(bot, settings);

  const [progress, setProgress] = useState(0);

  const getLastTransactionTimestamp = () => {
    if (isDisabled) return 0;
    const lastTransactionsTimestamp = ([...transactions]?.[0] || {}).created_at_timestamp;
    const lastSkippedTransactionTimestamp = ([...skippedTransactions]?.[0] || {}).created_at_timestamp;
    return Math.max(...[lastTransactionsTimestamp, lastSkippedTransactionTimestamp].filter(Number.isFinite));
  };

  const calculateProgress = () => {
    if (isDisabled) return 0;
    const now = moment();
    const nowTimestamp = now.unix();
    const lastTransactionTimestamp = getLastTransactionTimestamp();
    const prog = 1.0 - parseFloat(nextTransactionTimestamp - nowTimestamp)/(nextTransactionTimestamp - lastTransactionTimestamp);
    return prog * 100;
  };

  useInterval(() => {
    if (!isDisabled) {
      setProgress(calculateProgress());
    }
  }, 1000);

  return <ProgressBarLine colorClass={colorClass} progress={isDisabled ? 0 : progress} />;
});

ProgressBar.displayName = 'ProgressBar';
ProgressBarLine.displayName = 'ProgressBarLine';
