import React, { useState, useEffect } from 'react'
import moment from 'moment';
import { Line } from 'react-chartjs-2';
import API from '../../lib/API';
import { Spinner } from '../Spinner';
import { isEmpty } from '../../utils/array'

const TOTAL_INVESTED_CONFIG = {
        label: 'total invested',
        fill: false,
        lineTension: 0.4,
        backgroundColor: 'rgba(25,122,122,0.4)',
        borderColor: '#079e60',
        borderWidth: 2,
        borderCapStyle: 'butt',
        borderDash: [],
        borderDashOffset: 0.0,
        borderJoinStyle: 'miter',
        pointBorderColor: '#079e60',
        pointBackgroundColor: '#079e60',
        pointBorderWidth: 0,
        pointHoverRadius: 3,
        pointHoverBackgroundColor: '#079e60',
        pointHoverBorderColor: '#079e60',
        pointHoverBorderWidth: 2,
        pointRadius: 0,
        pointHitRadius: 10,
}

const VALUE_OVER_TIME_CONFIG = {
        label: 'value',
        fill: false,
        lineTension: 0.4,
        backgroundColor: 'rgba(75,192,192,0.4)',
        borderColor: '#22439E',
        borderWidth: 2,
        borderCapStyle: 'butt',
        borderDash: [],
        borderDashOffset: 0.0,
        borderJoinStyle: 'miter',
        pointBorderColor: '#22439E',
        pointBackgroundColor: '#22439E',
        pointBorderWidth: 0,
        pointHoverRadius: 3,
        pointHoverBackgroundColor: '#22439E',
        pointHoverBorderColor: '#22439E',
        pointHoverBorderWidth: 2,
        pointRadius: 0,
        pointHitRadius: 10,
}

export const Chart = ({bot}) => {
  const [data, setData] = useState([])

  useEffect(() => {
    loadData()
  }, [bot.nextTransactionTimestamp])

  const loadData = (id) => {
    API.getChartData(bot.id).then(({ data }) => {
      setData(data)
    })
  }

  if (isEmpty(data)) { return (<Spinner />) }

  const labels = data.map(el => el[0]).map(el => moment(el).format("DD-MM-YY"))
  const totalInvested = data.map(el => el[1])
  const currentValue = data.map(el => el[2])

  const chartData = {
    labels: labels,
    datasets: [
      {...TOTAL_INVESTED_CONFIG, data: totalInvested },
      {...VALUE_OVER_TIME_CONFIG, data: currentValue }
    ]
  }

  const chartOptions = {
    scales: {
      xAxes: [{
        ticks: {
          fontFamily: 'Inconsolata',
          fontColor: '#789'
        }
      }],
      yAxes: [{
        ticks: {
          fontFamily: 'Inconsolata',
          fontColor: '#789'
        }
      }]
    },
    legend: {
      labels: {
        padding: 44,
        fontFamily: 'Inconsolata',
        fontColor: '#789'
      }
    }
  }

  return (
    <div className="db-chart-container">
      <Line data={chartData} options={chartOptions} />
    </div>
  )
}
