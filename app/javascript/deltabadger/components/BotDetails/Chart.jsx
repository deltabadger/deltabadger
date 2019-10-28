import React from 'react'
import { Line } from 'react-chartjs-2';

export const Chart = ({data}) => {
  const labels = data.map(el => el[0])
  const totalInvested = data.map(el => el[1])
  const currentValue = data.map(el => el[2])

  const chartData = {
    labels: labels,
    datasets: [
      {
        label: 'total invested',
        fill: false,
        lineTension: 0.1,
        backgroundColor: 'rgba(25,122,122,0.4)',
        borderColor: 'rgba(75,12,112,1)',
        borderCapStyle: 'butt',
        borderDash: [],
        borderDashOffset: 0.0,
        borderJoinStyle: 'miter',
        pointBorderColor: 'rgba(75,192,172,1)',
        pointBackgroundColor: '#fff',
        pointBorderWidth: 1,
        pointHoverRadius: 5,
        pointHoverBackgroundColor: 'rgba(35,192,192,1)',
        pointHoverBorderColor: 'rgba(220,220,220,1)',
        pointHoverBorderWidth: 2,
        pointRadius: 1,
        pointHitRadius: 10,
        data: totalInvested
      },
      {
        label: 'current value',
        fill: false,
        lineTension: 0.1,
        backgroundColor: 'rgba(75,192,192,0.4)',
        borderColor: 'rgba(75,192,192,1)',
        borderCapStyle: 'butt',
        borderDash: [],
        borderDashOffset: 0.0,
        borderJoinStyle: 'miter',
        pointBorderColor: 'rgba(75,192,192,1)',
        pointBackgroundColor: '#fgf',
        pointBorderWidth: 1,
        pointHoverRadius: 5,
        pointHoverBackgroundColor: 'rgba(75,192,192,1)',
        pointHoverBorderColor: 'rgba(220,220,220,1)',
        pointHoverBorderWidth: 2,
        pointRadius: 1,
        pointHitRadius: 10,
        data: currentValue
      }
    ]
  }

  return (
    <div>
      <Line data={chartData} />
    </div>
  )
}
