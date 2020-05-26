import React from 'react'
import { Chart } from './Chart';

const Stats = ({
  bought,
  averagePrice,
  totalInvested,
  currentValue,
  profitLoss = {},
  currentPriceAvailable
}) => (
    <table className="table table-borderless db-table">
      <tbody>
        <tr>
          <td scope="col">Bought:</td>
          <th scope="col">{ bought }</th>
        </tr>
        <tr>
          <td scope="col">Average price:</td>
          <th scope="col">{ averagePrice }</th>
        </tr>
        <tr>
          <td scope="col">Total invested:</td>
          <th scope="col">{ totalInvested }</th>
        </tr>
        <tr>
          <td scope="col">Current value:</td>
          <th scope="col">
            { currentValue }
            { !currentPriceAvailable && <sup>*</sup> }
          </th>
        </tr>
        <tr>
          <td scope="col">Profit/Loss:</td>
          <th scope="col" className={`text-${profitLoss.positive ? 'success' : 'danger'}`}>
            { profitLoss.value }
            { !currentPriceAvailable && <sup>*</sup> }
          </th>
        </tr>
      </tbody>
    </table>
)

export const Transactions = ({ bot, active }) => (
  <div className={`tab-pane show ${active ? 'active' : ''}`} id="statistics" role="tabpanel" aria-labelledby="stats-tab">
    <Stats {...bot.stats} />
    { !bot.stats.currentPriceAvailable &&
      <p className="db-smallinfo">
       <i className="material-icons-round">error_outline</i> <sup>*</sup>
        The exchange API does not respond at the moment.
        <br/>
        Calculations are based on the last data available.
      </p>
    }

    <Chart bot={bot} />

    <table className="table table-borderless table-striped db-table db-table--tx">
      <thead>
        <tr>
          <th scope="col">Date</th>
          <th scope="col">Order</th>
          <th scope="col">Amount(BTC)</th>
          <th scope="col">Rate({bot.settings.currency})</th>
        </tr>
      </thead>
      <tbody>
        { bot.transactions.map(t => (
          <tr key={t.id} >
            <td scope="row">{t.created_at}</td>
            <td>{bot.settings.type}</td>
            <td>{t.amount || "N/A"}</td>
            <td>{parseFloat(t.rate).toFixed(2) || "N/A"}</td>
          </tr>
        ))}
      </tbody>
    </table>
    <p className="db-smallinfo"><i className="material-icons-round">error_outline</i> The table above shows only the last ten transactions.<br/>The full report is available to download in CSV format below.</p>
    <div className="db-bot-info__footer">
      <a href={`/api/bots/${bot.id}/transactions_csv`} className="btn btn-link btn--export-to-csv"><i className="material-icons-round mr-2">cloud_download</i><span> Download .csv</span> </a>
    </div>
  </div>
)
