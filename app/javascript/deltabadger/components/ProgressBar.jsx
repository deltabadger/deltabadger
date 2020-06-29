import React, { useState, useEffect } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';

const ProgressBarLine = ({colorClass, progress }) => (
  <div className="db-bot__progress progress progress--thin progress--bot-setup">
    <div className={`progress-bar bg-${colorClass}`} role="progressbar" style={{width: `${progress}%`, ariaValuenow: progress.toString(), ariaValuemin: "0", ariaValuemax: "100"}}></div>
  </div>
)

export const ProgressBar = ({bot}) => {
  const { settings, status, nextTransactionTimestamp, transactions } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const colorClass = settings.type == 'buy' ? 'success' : 'danger'
  const working = status == 'working'

  if (!working) { return <ProgressBarLine colorClass={colorClass} progress={0} /> }

  const [progress, setProgress] = useState(0)

  const calculateProgress = () => {
    const now  = new moment()
    const nowTimestamp = now.unix()
    const lastTransactionTimestamp = ([...transactions].shift() || {}).created_at_timestamp
    const prog = (nowTimestamp - lastTransactionTimestamp)/(nextTransactionTimestamp - lastTransactionTimestamp)
    return (prog) * 100
  }

  useInterval(() => {
    setProgress(calculateProgress())
  }, 1000);

  return (
    <div className="db-bot__progress progress progress--thin progress--bot-setup">
      <div className={`progress-bar bg-${colorClass}`} role="progressbar" style={{width: `${progress}%`, ariaValuenow: progress.toString(), ariaValuemin: "0", ariaValuemax: "100"}}></div>
    </div>
  )
}
