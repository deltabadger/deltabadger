import React from 'react'
import { Chart } from './Chart';

const Stats = ({ bought, avaragePrice, spent, currentValue, profitLoss = {} }) => (
    <table className="table table-borderless db-table">
      <tbody>
        <tr>
          <td scope="col">Bought:</td>
          <th scope="col">{ bought }</th>
        </tr>
        <tr>
          <td scope="col">Average price:</td>
          <th scope="col">{ avaragePrice }</th>
        </tr>
        <tr>
          <td scope="col">Spent:</td>
          <th scope="col">{ spent }</th>
        </tr>
        <tr>
          <td scope="col">Current value:</td>
          <th scope="col">{ currentValue }</th>
        </tr>
        <tr>
          <td scope="col">Profit/Loss:</td>
          <th scope="col" className={`text-${profitLoss.positive ? 'success' : 'danger'}`}>{ profitLoss.value }</th>
        </tr>
      </tbody>
    </table>
)

export const Transactions = ({ bot }) => (
  <div className="tab-pane show active" id="stats" role="tabpanel" aria-labelledby="stats-tab">
    <Stats {...bot.stats} />

    <Chart data={bot.charts.PortfolioValueOverTime} />

    <table className="table table-borderless table-striped db-table db-table--tx">
      <thead>
        <tr>
          <th scope="col">Date</th>
          <th scope="col">Action</th>
          <th scope="col">Amount(BTC)</th>
          <th scope="col">Price({bot.settings.currency})</th>
        </tr>
      </thead>
      <tbody>
        { bot.transactions.map(t => (
          <tr key={t.id} >
            <td scope="row">{t.created_at}</td>
            <td>{bot.settings.type}</td>
            <td>{t.amount || "N/A"}</td>
            <td>{t.price || "N/A"}</td>
          </tr>
        ))}
      </tbody>
    </table>
    <div className="db-bot-info__footer">
      <a href={`/api/bots/${bot.id}/transactions_csv`} className="btn btn-link btn--export-to-csv"><span>Export to .csv</span> <i className="material-icons">import_export</i></a>
    </div>
  </div>
)
