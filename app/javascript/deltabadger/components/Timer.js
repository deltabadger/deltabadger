import React, { useState } from 'react';
import moment from 'moment';
import { useInterval } from '../utils/interval';
import { formatDuration } from '../utils/time';

export const Timer = ({bot}) => {
  const { settings, status, nextTransactionTimestamp } = bot || {settings: {}, stats: {}, transactions: [], logs: []}
  const working = status == 'working'

  const calculateDelay = () => {
    const now = new moment()
    const date = nextTransactionTimestamp && new moment.unix(nextTransactionTimestamp)

    return nextTransactionTimestamp && moment.duration(date.diff(now))
  }

  const [delay, setDelay] = useState(calculateDelay())

  if (working) {
    useInterval(() => {
      setDelay(calculateDelay())
    }, 1000);

    return (
      <div className="db-bot__infotext__right">
        Next { settings.type } in { formatDuration(delay) }
      </div>
    )
  }
}
