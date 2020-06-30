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
    <p className="db-smallinfo">
      <svg className="db-svg-icon db-svg--inactive db-svg--table-disclaimer" xmlns="http://www.w3.org/2000/svg" width="24" viewBox="0 0 24 24"><path d="M12 7c.6 0 1 .5 1 1v4c0 .6-.5 1-1 1s-1-.5-1-1V8c0-.6.5-1 1-1zm0-5a10 10 0 100 20 10 10 0 000-20zm0 18a8 8 0 110-16 8 8 0 010 16zm1-3h-2v-2h2v2z"/></svg>
      The table above shows only the last ten transactions.<br/>The full report is available to download in CSV format below.
    </p>
    <div className="db-bot-info__footer">
      <a href={`/api/bots/${bot.id}/transactions_csv`} className="btn btn-link btn--export-to-csv">
      <svg className="btn__svg-icon db-svg-icon db-svg-icon--download" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M19.35 10.04A7.49 7.49 0 0012 4C9.11 4 6.6 5.64 5.35 8.04A5.994 5.994 0 000 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM17 13l-4.65 4.65c-.2.2-.51.2-.71 0L7 13h3V9h4v4h3z"/></svg>
      <span className="ml-3"> Download .csv</span> </a>
    </div>
  </div>
)
