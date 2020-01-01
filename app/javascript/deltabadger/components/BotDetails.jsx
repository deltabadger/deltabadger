import React from 'react'
import { Transactions } from './BotDetails/Transactions';
import { Logs } from './BotDetails/Logs';
import { Info } from './BotDetails/Info';
import { isNotEmpty, isEmpty } from '../utils/array';

export const BotDetails = ({ bot }) => {
  const tabs = [
    {
      label: "Statistics",
      active: isNotEmpty(bot.transactions),
      visible: isNotEmpty(bot.transactions),
      id: 'stats-tab',
    },
    {
      label: "Log",
      active: false,
      visible: isNotEmpty(bot.logs),
      id: 'log-tab',
    },
    {
      label: "Info",
      active: isEmpty(bot.transactions),
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
    <div className="db-bots__item db-bot-data">
      <ul className="nav nav-tabs" id="myTab" role="tablist">
        {tabs.filter(e => e.visible).map(builTab)}
      </ul>
      <div className="tab-content" id="myTabContent">
        <Transactions bot={bot} active={isNotEmpty(bot.transactions)} />
        <Logs bot={bot} />
        <Info bot={bot} active={isEmpty(bot.transactions)} />
      </div>

    </div>
  )
}
