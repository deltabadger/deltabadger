import React from 'react'
import I18n from 'i18n-js'

export const Logs = ({ bot, active }) => (
  <div className={`legacy-tab ${active ? 'active' : ''}`} id="log" role="tabpanel" aria-labelledby="log-tab">
    <table className="table table-striped table-borderless db-table db-table--tx">
      <thead>
        <tr>
          <th scope="col">{I18n.t('bot.details.log.date')}</th>
          <th scope="col">{I18n.t('bot.details.log.errors')}</th>
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
