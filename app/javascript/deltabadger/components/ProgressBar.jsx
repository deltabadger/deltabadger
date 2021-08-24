import React, { useState } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';

const ProgressBarLine = ({colorClass, progress }) => (
  <div className="db-bot__progress progress progress--thin progress--bot-setup">
    <div className={`progress-bar bg-${colorClass}`} role="progressbar" style={{width: `${progress}%`}} aria-valuenow={Math.round(progress)} aria-valuemin="0" aria-valuemax="100" />
  </div>
)

export const ProgressBar = ({bot}) => {
  const { settings, status, nextTransactionTimestamp, transactions, skippedTransactions} = bot || {settings: {}, stats: {}, transactions: [], skippedTransactions: [], logs: []}
  const colorClass = settings.type == 'buy' ? 'success' : 'danger'
  const working = status == 'working'

  if (!working) { return <ProgressBarLine colorClass={colorClass} progress={0} /> }

  const [progress, setProgress] = useState(0)

  const getLastTransactionTimestamp = () => {
    const lastTransactionsTimestamp = ([...transactions].shift() || {}).created_at_timestamp
    const lastSkippedTransactionTimestamp = ([...skippedTransactions].shift() || {}).created_at_timestamp

    return Math.max(...[lastTransactionsTimestamp, lastSkippedTransactionTimestamp].filter(Number.isFinite))
  }

  const calculateProgress = () => {
    const now  = new moment()
    const nowTimestamp = now.unix()
    const lastTransactionTimestamp = getLastTransactionTimestamp()
    const prog = (nowTimestamp - lastTransactionTimestamp)/(nextTransactionTimestamp - lastTransactionTimestamp)
    return (prog) * 100
  }

  useInterval(() => {
    setProgress(calculateProgress())
  }, 1000);

  return <ProgressBarLine colorClass={colorClass} progress={progress} />
}
