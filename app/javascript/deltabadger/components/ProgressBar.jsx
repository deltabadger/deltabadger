import React, { useState, useEffect, useCallback } from 'react';
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

  const getLastTransactionTimestamp = useCallback(() => {
    if (isDisabled) return 0;
    const lastTransactionsTimestamp = ([...transactions]?.[0] || {}).created_at_timestamp;
    const lastSkippedTransactionTimestamp = ([...skippedTransactions]?.[0] || {}).created_at_timestamp;
    return Math.max(...[lastTransactionsTimestamp, lastSkippedTransactionTimestamp].filter(Number.isFinite));
  }, [isDisabled, transactions, skippedTransactions]);

  const calculateProgress = useCallback(() => {
    if (isDisabled) return 0;
    const now = moment();
    const nowTimestamp = now.unix();
    const lastTransactionTimestamp = getLastTransactionTimestamp();
    
    // Ensure we have valid timestamps
    if (!nextTransactionTimestamp || !lastTransactionTimestamp) return 0;
    
    const prog = 1.0 - parseFloat(nextTransactionTimestamp - nowTimestamp)/(nextTransactionTimestamp - lastTransactionTimestamp);
    return Math.max(0, Math.min(100, prog * 100)); // Clamp between 0 and 100
  }, [isDisabled, nextTransactionTimestamp, getLastTransactionTimestamp]);

  // Initial sync on mount
  useEffect(() => {
    setProgress(calculateProgress());
  }, []); // Empty dependency array for mount only

  // Handle visibility change
  useEffect(() => {
    const handleVisibilityChange = () => {
      if (!document.hidden) {
        // Recalculate progress when becoming visible
        setProgress(calculateProgress());
      }
    };

    // Initial sync
    setProgress(calculateProgress());

    document.addEventListener('visibilitychange', handleVisibilityChange);
    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, [calculateProgress]);

  // Update progress when bot data changes
  useEffect(() => {
    setProgress(calculateProgress());
  }, [bot, calculateProgress]);

  // Handle interval updates
  useInterval(() => {
    if (!isDisabled && !document.hidden) {
      setProgress(calculateProgress());
    }
  }, 1000);

  // return <ProgressBarLine colorClass={colorClass} progress={isDisabled ? 0 : progress} />;
});

ProgressBar.displayName = 'ProgressBar';
ProgressBarLine.displayName = 'ProgressBarLine';
