import React, { useState, useEffect } from 'react'
import I18n from 'i18n-js'
import moment from 'moment';
import 'chart.js/auto';
import { Chart as ReactChart } from 'react-chartjs-2';
import API from '../../lib/API';
import { Spinner } from '../Spinner';
import { isEmpty } from '../../utils/array'
import {renameSymbol, shouldRename} from "../../utils/symbols";

const TOTAL_INVESTED_CONFIG = {
        fill: false,
        lineTension: 0.4,
        backgroundColor: 'rgba(25,122,122,0.4)',
        borderColor: '#00b186',
        borderWidth: 2,
        borderCapStyle: 'butt',
        borderDash: [],
        borderDashOffset: 0.0,
        borderJoinStyle: 'miter',
        pointBorderColor: '#00b186',
        pointBackgroundColor: '#00b186',
        pointBorderWidth: 0,
        pointHoverRadius: 3,
        pointHoverBackgroundColor: '#00b186',
        pointHoverBorderColor: '#00b186',
        pointHoverBorderWidth: 2,
        pointRadius: 0,
        pointHitRadius: 10,
}

const VALUE_OVER_TIME_CONFIG = {
        fill: false,
        lineTension: 0.4,
        backgroundColor: 'rgba(75,192,192,0.4)',
        borderColor: '#3457b9',
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

  const { exchangeName } = bot;
  const base = shouldRename(exchangeName) ? renameSymbol(bot.settings.base) : bot.settings.base
  const value_label = isBuyingBot(bot) ? I18n.t('bots.details.stats.current_value') : I18n.t('bots.details.stats.current_value_sold', {base: base})
  const invested_label = isBuyingBot(bot) ? I18n.t('bots.details.stats.chart.total_invested') : I18n.t('bots.details.stats.bought')
  const chartData = {
    labels: labels,
    datasets: [
      {...TOTAL_INVESTED_CONFIG,  label: invested_label, data: totalInvested },
      {...VALUE_OVER_TIME_CONFIG, label: value_label, data: currentValue }
    ]
  }

  const chartOptions = {
    scales: {
      x: {
        ticks: {
          fontFamily: 'Montserrat',
          fontColor: '#789'
        }
      },
      y: {
        ticks: {
          fontFamily: 'Montserrat',
          fontColor: '#789'
        }
      }
    },
    plugins: {
      legend: {
        labels: {
          padding: 44,
          fontFamily: 'Montserrat',
          fontColor: '#789',
          usePointStyle: true,
          boxWidth: 7,
          boxHeight: 7,
        }
      }
    }
  }

  return (
    <div className="db-chart-container">
      <ReactChart type='line' data={chartData} options={chartOptions} />
    </div>
  )
}

const isBuyingBot = (bot) => {
    return bot.settings.type === 'buy'
}
