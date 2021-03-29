import React from 'react'
import I18n from 'i18n-js'
import { RawHTML } from '../RawHtml'
import { Chart } from './Chart';
import { shouldRename, renameSymbol } from '../../utils/symbols';

const Stats = ({
  base,
  quote,
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
          <td scope="col">{ I18n.t('bots.details.stats.bought') }:</td>
          <th scope="col">{ toFixedWithoutZeros(bought) } { base }</th>
        </tr>
        <tr>
          <td scope="col">{ I18n.t('bots.details.stats.average_price') }:</td>
          <th scope="col">{ toFixedWithoutZeros(averagePrice) } { quote }</th>
        </tr>
        <tr>
          <td scope="col">{ I18n.t('bots.details.stats.total_invested') }:</td>
          <th scope="col">{ toFixedWithoutZeros(totalInvested) } { quote }</th>
        </tr>
        <tr>
          <td scope="col">{ I18n.t('bots.details.stats.current_value') }:</td>
          <th scope="col">
            { toFixedWithoutZeros(currentValue) } { quote }
            { !currentPriceAvailable && <sup>*</sup> }
          </th>
        </tr>
        <tr>
          <td scope="col">{ I18n.t('bots.details.stats.profit_loss') }:</td>
          <th scope="col" className={`text-${profitLoss.positive ? 'success' : 'danger'}`}>
            { toFixedWithoutZeros(profitLoss.value) } { quote } { profitLoss.percentage }
            { !currentPriceAvailable && <sup>*</sup> }
          </th>
        </tr>
      </tbody>
    </table>
)

export const Transactions = ({ bot, active }) => {
  const { exchangeName } = bot;
  const base = shouldRename(exchangeName) ? renameSymbol(bot.settings.base) : bot.settings.base
  const quote = shouldRename(exchangeName) ? renameSymbol(bot.settings.quote) : bot.settings.quote
  return <div className={`tab-pane show ${active ? 'active' : ''}`} id="statistics" role="tabpanel" aria-labelledby="stats-tab">
    <Stats base={base} quote={quote} {...bot.stats} />
    { !bot.stats.currentPriceAvailable &&
      <p className="db-smallinfo">
        <svg className="db-svg-icon db-svg--inactive db-svg--table-disclaimer" xmlns="http://www.w3.org/2000/svg" width="2.4rem" viewBox="0 0 24 24">
          <path d="M12 7c.6 0 1 .5 1 1v4c0 .6-.5 1-1 1s-1-.5-1-1V8c0-.6.5-1 1-1zm0-5a10 10 0 100 20 10 10 0 000-20zm0 18a8 8 0 110-16 8 8 0 010 16zm1-3h-2v-2h2v2z"/>
        </svg>
        <sup>*</sup>
        <RawHTML tag="span">{I18n.t('bots.details.stats.exchange_unavailable_html')}</RawHTML>
      </p>
    }

    <Chart bot={bot} />

    <table className="table table-borderless table-striped db-table db-table--tx">
      <thead>
        <tr>
          <th scope="col">{ I18n.t('bots.details.stats.date') }</th>
          <th scope="col">{ I18n.t('bots.details.stats.order') }</th>
          <th scope="col">{ I18n.t('bots.details.stats.amount', { base }) }</th>
          <th scope="col">{ I18n.t('bots.details.stats.rate', { quote }) }</th>
        </tr>
      </thead>
      <tbody>
        { bot.transactions.map(t => (
          <tr key={t.id} >
            <td scope="row">{t.created_at}</td>
            <td>{translateBuyOrSell(bot.settings.type).toLowerCase()}</td>
            <td>{toFixedWithoutZeros(t.amount) || "N/A"}</td>
            <td>{toFixedWithoutZeros(t.rate) || "N/A"}</td>
          </tr>
        ))}
      </tbody>
    </table>
    <p className="db-smallinfo">
      <svg className="db-svg-icon db-svg--inactive db-svg--table-disclaimer" xmlns="http://www.w3.org/2000/svg" width="2.4rem" viewBox="0 0 24 24">
        <path d="M12 7c.6 0 1 .5 1 1v4c0 .6-.5 1-1 1s-1-.5-1-1V8c0-.6.5-1 1-1zm0-5a10 10 0 100 20 10 10 0 000-20zm0 18a8 8 0 110-16 8 8 0 010 16zm1-3h-2v-2h2v2z"/>
      </svg>
      <RawHTML tag="span">{I18n.t('bots.details.stats.csv_download_html')}</RawHTML>
    </p>
    <div className="db-bot-info__footer">
      <a href={`/api/bots/${bot.id}/transactions_csv`} className="btn btn-link btn--export-to-csv">
      <svg className="btn__svg-icon db-svg-icon db-svg-icon--download" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M19.35 10.04A7.49 7.49 0 0012 4C9.11 4 6.6 5.64 5.35 8.04A5.994 5.994 0 000 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM17 13l-4.65 4.65c-.2.2-.51.2-.71 0L7 13h3V9h4v4h3z"/></svg>
      <span> {I18n.t('bots.details.stats.download_csv')}</span> </a>
    </div>
  </div>
}

const toFixedWithoutZeros = (x) => {
  if(parseFloat(x) >= 1.0 || parseFloat(x) <= -1.0){
    return parseFloat(x).toFixed(2);
  }

  return parseFloat(x).toFixed(8).replace(/\.?0*$/,'');
}

const translateBuyOrSell = (side) => {
  return side == 'buy' ? I18n.t('bots.buy') : I18n.t('bots.sell')
}
