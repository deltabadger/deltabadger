import React from 'react'
import { Transactions } from './BotDetails/Transactions';
import { Logs } from './BotDetails/Logs';
import { Info } from './BotDetails/Info';

export const BotDetails = ({ bot }) => {
  return (
    <div className="db-bots__item db-bot-data">
      <ul className="nav nav-tabs" id="myTab" role="tablist">
        <li className="nav-item">
          <a className="nav-link active" id="stats-tab" data-toggle="tab" href="#stats" role="tab" aria-controls="stats" aria-selected="true">Statistics</a>
        </li>
        <li className="nav-item">
          <a className="nav-link"        id="log-tab"   data-toggle="tab" href="#log"   role="tab" aria-controls="log"   aria-selected="false">Log</a>
        </li>
        <li className="nav-item">
          <a className="nav-link"        id="info-tab"  data-toggle="tab" href="#info"  role="tab" aria-controls="info"  aria-selected="false">Info</a>
        </li>
      </ul>
      <div className="tab-content" id="myTabContent">
        <Transactions bot={bot} />
        <Logs bot={bot} />
        <Info bot={bot} />
      </div>
    </div>
  )
}
