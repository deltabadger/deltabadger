import React from 'react'

export const Transactions = ({ bot }) => (
  <div className="tab-pane show active" id="stats" role="tabpanel" aria-labelledby="stats-tab">

    <table className="table table-borderless table-sm db-table db-table--tx">
      <thead>
        <tr>
          <th scope="col">Date</th>
          <th scope="col">Exchange</th>
          <th scope="col">Action</th>
          <th scope="col">Amount(BTC)</th>
          <th scope="col">Price({bot.settings.currency})</th>
        </tr>
      </thead>
      <tbody>
        { bot.transactions.map(t => (
          <tr key={t.id} >
            <th scope="row">{t.created_at}</th>
            <td>{bot.exchangeName}</td>
            <td>{bot.settings.type}</td>
            <td>{t.amount || "N/A"}</td>
            <td>{t.price || "N/A"}</td>
          </tr>
        ))}
      </tbody>
    </table>
    <div className="db-bot-info__footer">
      <a href={`/api/bots/${bot.id}/transactions_csv`} className="btn btn-link btn--export-to-csv">Export to .csv <i className="fas fa-download ml-1"></i></a>
    </div>
  </div>
)
