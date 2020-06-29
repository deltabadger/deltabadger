import React from 'react'
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
      label: "Statistics",
      active: statisticsActive,
      visible: isNotEmpty(bot.transactions),
      id: 'stats-tab',
    },
    {
      label: "Log",
      active: logActive,
      visible: isNotEmpty(bot.logs),
      id: 'log-tab',
    },
    {
      label: "Info",
      active: infoActive,
      visible: true,
      id: 'info-tab',
    }
  ];


  const builTab = ({ label, active, visible, id }, index) => (
    <li className="nav-item" key={index}>
      <a className={`nav-link ${active ? 'active' : ''}`} id={id} data-toggle="tab" href={`#${label.toLowerCase()}`} role="tab" aria-controls={label.toLowerCase()} aria-selected="true">{label}</a>
    </li>
  )

  return (
    <div className="db-bots__item db-bots__item--data">
      <ul className="nav nav-tabs" id="myTab" role="tablist">
        {tabs.filter(e => e.visible).map(builTab)}
      </ul>
      <div className="tab-content" id="myTabContent">
        <Transactions bot={bot} active={statisticsActive} />
        <Logs bot={bot} active={logActive}/>
        <Info bot={bot} active={infoActive} />
      </div>

    </div>
  )
}
