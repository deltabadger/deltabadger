import React from 'react'
import I18n from 'i18n-js'
import { Transactions } from './BotDetails/Transactions';
import { Logs } from './BotDetails/Logs';
import { Info } from './BotDetails/Info';
import { isNotEmpty, isEmpty } from '../utils/array';

export const BotDetails = ({ bot }) => {
  const statisticsActive = isNotEmpty(bot.transactions)
  const logActive = isNotEmpty(bot.logs) && !statisticsActive
  const infoActive = isEmpty(bot.transactions) && isEmpty(bot.logs)

  const tabs = [
    {
      label: I18n.t('bots.details.stats.title'),
      active: statisticsActive,
      visible: isNotEmpty(bot.transactions),
      id: 'stats-tab',
      tabpanelId: 'statistics',
    },
    {
      label: I18n.t('bots.details.log.title'),
      active: logActive,
      visible: isNotEmpty(bot.logs),
      id: 'log-tab',
      tabpanelId: 'log',
    },
    {
      label: I18n.t('bots.details.info.title'),
      active: infoActive,
      visible: true,
      id: 'info-tab',
      tabpanelId: 'info',
    }
  ];


  const buildTab = ({ label, active, id, tabpanelId }, index) => (
    <li className="nav-item" key={index}>
      <a
        className={`nav-link ${active ? 'active' : ''}`}
        id={id}
        data-toggle="tab"
        href={`#${tabpanelId}`}
        role="tab"
        aria-controls={tabpanelId}
        aria-selected={active}
      >
        {label}
      </a>
    </li>
  )

  return (
    <div className="db-bots__item db-bots__item--data">
      <ul className="nav nav-tabs" role="tablist">
        {tabs.filter(e => e.visible).map(buildTab)}
      </ul>
      <div className="tab-content">
        <Transactions bot={bot} active={statisticsActive} />
        <Logs bot={bot} active={logActive}/>
        <Info bot={bot} active={infoActive} />
      </div>
    </div>
  )
}
