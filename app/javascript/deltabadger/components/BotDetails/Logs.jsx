import React from 'react'

export const Logs = ({ bot }) => (
  <div className="tab-pane" id="log" role="tabpanel" aria-labelledby="log-tab">
    <table className="table table-sm db-table db-table--tx">
      <thead>
        <tr>
          <th scope="col">Date</th>
          <th scope="col">Errors</th>
        </tr>
      </thead>
      <tbody>
        { bot.logs.map(t => (
          <tr key={t.id} >
            <th scope="row">{t.created_at}</th>
            <td>{t.errors}</td>
          </tr>
        ))}
      </tbody>
    </table>
  </div>
)
